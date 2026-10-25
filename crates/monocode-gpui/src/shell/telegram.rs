//! Telegram questions in the app: one bridge per process, a driver that forwards the
//! questions agents leave unanswered, routes the answers back to the session that asked, and
//! the menu items that open the two Telegram dialogs.
//!
//! The forwarding rules live in `monocode_core::telegram::{forward, forwarder}`. This file
//! owns the clock, the sessions and the background work: every bridge call runs on the
//! background executor, and the answers come in through a channel the UI drains.
use super::*;
use crate::conversation::{QuestionAsked, QuestionSettled};
use crate::telegram_dialog::{self, LinkTopicDialog, TelegramDialog};
use gpui::{AnyView, Task};
use monocode_core::{
    session_groups::WorkItem,
    telegram::{
        self, ANSWER_EVENT, Bridge, EventSink, ForwardPayload, TelegramStatus,
        forward::{Block, Pending, SessionInfo, is_topic_linkable, project_key, request_id_of},
        forwarder::{Action, Forwarder, ProjectStatus, StatusCache, TICK_MS},
    },
};
use serde_json::Value;
use std::path::PathBuf;

/// The label answers are routed to. The app has one shell window, which owns every session.
pub(super) const WINDOW: &str = "main";

/// What the driver asks of Telegram. The bridge implements it; tests record the calls.
pub(super) trait Wire: Send + Sync {
    fn status(&self, project: &str) -> TelegramStatus;
    fn forward(&self, payload: ForwardPayload) -> Result<(), String>;
    fn closed(&self, session_id: &str, request_id: u64, status: &str);
    fn answered(&self, key: &str, ok: bool);
}

impl Wire for Bridge {
    fn status(&self, project: &str) -> TelegramStatus {
        telegram::status(self, project)
    }
    fn forward(&self, payload: ForwardPayload) -> Result<(), String> {
        telegram::forward_question(self, WINDOW, payload)
    }
    fn closed(&self, session_id: &str, request_id: u64, status: &str) {
        let _ = telegram::question_closed(self, session_id, request_id, status);
    }
    fn answered(&self, key: &str, ok: bool) {
        telegram::answer_result(self, key, ok);
    }
}

/// Hands an answer from the poller thread to the UI loop. It never blocks: the channel is
/// unbounded, and once the shell is gone the send fails, so the answer counts as failed.
struct AppSink {
    sender: async_channel::Sender<Value>,
}

impl EventSink for AppSink {
    fn emit_to(&self, window: &str, event: &str, payload: Value) -> bool {
        window == WINDOW && event == ANSWER_EVENT && self.sender.try_send(payload).is_ok()
    }
}

type Job = Box<dyn FnOnce() + Send>;

pub(super) struct OpenDialog {
    view: Entity<TelegramDialog>,
    _events: Subscription,
}

pub(super) struct OpenTopic {
    view: Entity<LinkTopicDialog>,
    _events: Subscription,
}

pub(super) struct Telegram {
    /// What the driver talks to; none until the bridge is up (and in most tests).
    wire: Option<Arc<dyn Wire>>,
    /// The bridge the dialogs use.
    bridge: Option<Arc<Bridge>>,
    sender: async_channel::Sender<Value>,
    receiver: Option<async_channel::Receiver<Value>>,
    /// Calls whose result nobody waits for, run in order on one background task: a verdict
    /// on an answer must not overtake an earlier one for the same key.
    outbox: Option<async_channel::Sender<Job>>,
    outbox_task: Option<Task<()>>,
    forwarder: Forwarder,
    cache: StatusCache,
    /// Bumped when settings change, so a status read that started before is not cached.
    epoch: u64,
    /// Whether a pass is running, and whether another was asked for meanwhile.
    passing: bool,
    again: bool,
    /// Projects (by key) that have a bot and a connected group, for the session menu.
    linkable: HashMap<String, bool>,
    pub(super) dialog: Option<OpenDialog>,
    pub(super) topic: Option<OpenTopic>,
    tick: Option<Task<()>>,
    drain: Option<Task<()>>,
}

impl Telegram {
    pub(super) fn new() -> Self {
        let (sender, receiver) = async_channel::unbounded();
        Self {
            wire: None,
            bridge: None,
            sender,
            receiver: Some(receiver),
            outbox: None,
            outbox_task: None,
            forwarder: Forwarder::default(),
            cache: StatusCache::default(),
            epoch: 0,
            passing: false,
            again: false,
            linkable: HashMap::new(),
            dialog: None,
            topic: None,
            tick: None,
            drain: None,
        }
    }

    pub(super) fn dialog_open(&self) -> bool {
        self.dialog.is_some() || self.topic.is_some()
    }
}

/// What a pass saw: the sessions with a pending question, and the local id of each.
pub(super) struct Snapshots {
    infos: Vec<SessionInfo>,
    ids: HashMap<String, i64>,
}

/// The number a session gets in its Telegram topic: only a linked GitHub issue or pull
/// request has one; tracker items (Linear, Jira, ClickUp) are named by their identifier.
fn github_number(item: &WorkItem) -> Option<u64> {
    matches!(item.kind.as_str(), "issue" | "pr").then_some(item.number)
}

impl Shell {
    /// Starts draining answers, and the bridge once per process (never in tests, whose
    /// shells install their own wire).
    pub(super) fn telegram_start(&mut self, data_dir: Option<PathBuf>, cx: &mut Context<Self>) {
        if let Some(receiver) = self.telegram.receiver.take() {
            self.telegram.drain = Some(cx.spawn(async move |this, cx| {
                while let Ok(payload) = receiver.recv().await {
                    if this
                        .update(cx, |this, cx| this.telegram_answer(payload, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        let Some(dir) = data_dir.filter(|_| !cfg!(test)) else {
            return;
        };
        let sink = Arc::new(AppSink {
            sender: self.telegram.sender.clone(),
        });
        cx.spawn(async move |this, cx| {
            let bridge = cx
                .background_executor()
                .spawn(async move { telegram::init(&dir, sink) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.telegram_ready(bridge.clone(), Some(bridge), cx)
            });
        })
        .detach();
    }

    /// The bridge is up: start the 15 s tick and look at what is already waiting.
    pub(super) fn telegram_ready(
        &mut self,
        wire: Arc<dyn Wire>,
        bridge: Option<Arc<Bridge>>,
        cx: &mut Context<Self>,
    ) {
        self.telegram.wire = Some(wire);
        self.telegram.bridge = bridge;
        let (outbox, jobs) = async_channel::unbounded::<Job>();
        self.telegram.outbox = Some(outbox);
        self.telegram.outbox_task = Some(cx.background_executor().spawn(async move {
            while let Ok(job) = jobs.recv().await {
                job();
            }
        }));
        self.telegram.tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(TICK_MS))
                    .await;
                let ticked = this.update(cx, |this, cx| {
                    this.telegram_poke(cx);
                    this.telegram_refresh_linkable(cx);
                });
                if ticked.is_err() {
                    break;
                }
            }
        }));
        self.telegram_poke(cx);
        self.telegram_refresh_linkable(cx);
    }

    /// Everything the forwarder needs to know about one session. No question and no
    /// message: `telegram_snapshots` adds those for the sessions that are asking.
    pub(super) fn telegram_info(&self, id: i64, cx: &App) -> Option<SessionInfo> {
        if !self.sessions.contains_key(&id) && !self.session_summaries.contains_key(&id) {
            return None;
        }
        let summary = &self.session_metadata(id, cx).summary;
        // A remote session is never forwarded or linked, and its persisted id costs a
        // database read: do not ask for it.
        let persisted = if summary
            .cwd
            .starts_with(monocode_core::remote::REMOTE_PATH_PREFIX)
        {
            id.to_string()
        } else {
            locked(&self.store).ok()?.persisted_session_id(id).ok()?
        };
        Some(SessionInfo {
            id: persisted,
            cwd: summary.cwd.clone(),
            title: summary.title.clone(),
            provider: summary.provider.clone(),
            surface: summary.surface,
            orchestration_worker: self.orchestration_worker(id),
            mono: self
                .monos
                .as_ref()
                .is_some_and(|monos| monos.read(cx).is_mono_session(id)),
            ticket: self.session_tickets.get(&id).cloned(),
            linked_number: summary.work_item.as_ref().and_then(github_number),
            ..Default::default()
        })
    }

    /// The sessions that are asking something right now.
    pub(super) fn telegram_snapshots(&self, cx: &App) -> Snapshots {
        let mut snapshots = Snapshots {
            infos: Vec::new(),
            ids: HashMap::new(),
        };
        for (&id, chat) in &self.sessions {
            let chat = chat.read(cx);
            let Some(question) = chat.pending_question(cx) else {
                continue;
            };
            let Some(mut info) = self.telegram_info(id, cx) else {
                continue;
            };
            info.blocks = vec![Block {
                role: "assistant".into(),
                text: chat.last_assistant_text(cx),
                internal: false,
            }];
            info.pending = Some(Pending::from_question(&question));
            snapshots.ids.insert(info.id.clone(), id);
            snapshots.infos.push(info);
        }
        snapshots
    }

    /// Looks at the questions now: reports closed ones, reads the statuses it needs (off the
    /// UI thread, through the cache) and forwards those that are due.
    pub(super) fn telegram_poke(&mut self, cx: &mut Context<Self>) {
        if self.telegram.wire.is_none() {
            return;
        }
        if std::mem::replace(&mut self.telegram.passing, true) {
            self.telegram.again = true;
            return;
        }
        self.telegram_pass(cx);
    }

    fn telegram_pass(&mut self, cx: &mut Context<Self>) {
        let Some(wire) = self.telegram.wire.clone() else {
            return;
        };
        let (now, epoch) = (monocode_core::time::now_ms(), self.telegram.epoch);
        let snapshots = self.telegram_snapshots(cx);
        let closes = self.telegram.forwarder.sync(now, &snapshots.infos);
        let mut statuses = HashMap::new();
        let mut missing = Vec::new();
        for project in self.telegram.forwarder.projects_to_read(&snapshots.infos) {
            match self.telegram.cache.get(&project, now) {
                Some(status) => {
                    statuses.insert(project, status.clone());
                }
                None => missing.push(project),
            }
        }
        self.telegram_act(closes, &snapshots, cx);
        if missing.is_empty() {
            self.telegram_forward(statuses, cx);
            self.telegram_pass_done(cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    missing
                        .into_iter()
                        .map(|project| {
                            let status = ProjectStatus::from(&wire.status(&project));
                            (project, status)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let now = monocode_core::time::now_ms();
                for (project, status) in read {
                    if this.telegram.epoch == epoch {
                        this.telegram.cache.put(&project, now, status.clone());
                    }
                    statuses.insert(project, status);
                }
                this.telegram_forward(statuses, cx);
                this.telegram_pass_done(cx);
            });
        })
        .detach();
    }

    /// Forwards what is due, against the sessions as they are now.
    fn telegram_forward(
        &mut self,
        statuses: HashMap<String, ProjectStatus>,
        cx: &mut Context<Self>,
    ) {
        let snapshots = self.telegram_snapshots(cx);
        let now = monocode_core::time::now_ms();
        let actions = self
            .telegram
            .forwarder
            .forward(now, &snapshots.infos, &statuses);
        self.telegram_act(actions, &snapshots, cx);
    }

    fn telegram_pass_done(&mut self, cx: &mut Context<Self>) {
        self.telegram.passing = false;
        if std::mem::take(&mut self.telegram.again) {
            self.telegram_poke(cx);
        }
    }

    /// Carries out what the forwarder asked for. `snapshots` are the sessions the actions
    /// were computed from, so an answer lands on the session it was checked against.
    fn telegram_act(
        &mut self,
        actions: Vec<Action>,
        snapshots: &Snapshots,
        cx: &mut Context<Self>,
    ) {
        let Some(wire) = self.telegram.wire.clone() else {
            return;
        };
        for action in actions {
            match action {
                Action::Forward(payload) => {
                    let wire = wire.clone();
                    let (session, request) = (payload.session_id.clone(), payload.request_id);
                    cx.spawn(async move |this, cx| {
                        let ok = cx
                            .background_executor()
                            .spawn(async move { wire.forward(payload).is_ok() })
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            let snapshots = this.telegram_snapshots(cx);
                            let now = monocode_core::time::now_ms();
                            let actions = this.telegram.forwarder.forward_done(
                                now,
                                &snapshots.infos,
                                &session,
                                request,
                                ok,
                            );
                            this.telegram_act(actions, &snapshots, cx);
                        });
                    })
                    .detach();
                }
                Action::Close {
                    session_id,
                    request_id,
                    status,
                } => {
                    let wire = wire.clone();
                    self.telegram_later(move || wire.closed(&session_id, request_id, status));
                }
                Action::Apply(apply) => {
                    let chat = snapshots
                        .ids
                        .get(&apply.session_id)
                        .and_then(|id| self.sessions.get(id))
                        .cloned();
                    let ok = chat.is_some_and(|chat| {
                        chat.update(cx, |chat, cx| {
                            chat.answer_question(&apply.question_id, apply.answers.clone(), cx)
                        })
                        .is_ok()
                    });
                    let result = self.telegram.forwarder.answer_applied(&apply, ok);
                    self.telegram_act(vec![result], snapshots, cx);
                }
                Action::AnswerResult { key, ok } => {
                    let wire = wire.clone();
                    self.telegram_later(move || wire.answered(&key, ok));
                }
            }
        }
    }

    /// Runs a call off the UI thread, after the ones queued before it.
    fn telegram_later(&self, job: impl FnOnce() + Send + 'static) {
        if let Some(outbox) = &self.telegram.outbox {
            let _ = outbox.try_send(Box::new(job));
        }
    }

    /// An answer from the Telegram group. The sessions are read at this moment and the answer
    /// is applied before anything else can run, so a second reply to the same question finds
    /// it gone.
    pub(super) fn telegram_answer(&mut self, payload: Value, cx: &mut Context<Self>) {
        if self.telegram.wire.is_none() {
            return;
        }
        let snapshots = self.telegram_snapshots(cx);
        let actions = self
            .telegram
            .forwarder
            .on_answer_json(&payload, &snapshots.infos);
        self.telegram_act(actions, &snapshots, cx);
    }

    /// Watches one session for questions asked and settled.
    pub(super) fn observe_telegram(
        &mut self,
        chat: &Entity<Conversation>,
        id: i64,
        cx: &mut Context<Self>,
    ) {
        self.observations
            .push(cx.subscribe(chat, |this, _, _: &QuestionAsked, cx| {
                this.telegram_poke(cx)
            }));
        self.observations.push(cx.subscribe(
            chat,
            move |this, _, settled: &QuestionSettled, cx| {
                // Before the next snapshot stops listing the question.
                let key = locked(&this.store)
                    .ok()
                    .and_then(|store| store.persisted_session_id(id).ok());
                if let Some(key) = key {
                    this.telegram.forwarder.record_outcome(
                        &key,
                        request_id_of(&settled.id),
                        settled.outcome,
                    );
                }
                this.telegram_poke(cx);
            },
        ));
    }

    /// A project's Telegram settings changed: forget the status read before and look again.
    pub(super) fn telegram_settings_changed(&mut self, project: &str, cx: &mut Context<Self>) {
        self.telegram.epoch += 1;
        self.telegram.cache.invalidate(Some(&project_key(project)));
        self.telegram_poke(cx);
        self.telegram_refresh_linkable(cx);
    }

    /// Reads which projects have a bot and a group, for the session menu.
    pub(super) fn telegram_refresh_linkable(&mut self, cx: &mut Context<Self>) {
        let Some(wire) = self.telegram.wire.clone() else {
            return;
        };
        let mut projects: Vec<String> = self
            .workspace
            .projects
            .iter()
            .map(|project| project_key(project))
            .collect();
        projects.sort();
        projects.dedup();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    projects
                        .into_iter()
                        .map(|project| {
                            let status = wire.status(&project);
                            (project, status.configured && status.chat_id.is_some())
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, _| this.telegram.linkable.extend(read));
        })
        .detach();
    }

    /// "Link Telegram topic…", offered when the session's project has a bot and a group.
    pub(super) fn telegram_topic_item(&self, id: i64, cx: &App) -> Option<MenuItem> {
        let info = self.telegram_info(id, cx)?;
        let linkable = *self.telegram.linkable.get(&project_key(&info.cwd))?;
        (linkable && is_topic_linkable(&info))
            .then(|| MenuAction::new("link-telegram-topic", "Link Telegram topic…").into())
    }

    pub(super) fn open_telegram_dialog(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        self.close_preview(cx);
        let Some(bridge) = self.telegram.bridge.clone() else {
            self.error = Some("Telegram is not available yet.".into());
            cx.notify();
            return;
        };
        let project = project_key(&path);
        let name = self.project_label(&path);
        let view = cx.new(|cx| TelegramDialog::new(project.clone(), name, bridge, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &telegram_dialog::Event, window, cx| match event {
                telegram_dialog::Event::Close => this.close_telegram_dialog(window, cx),
                telegram_dialog::Event::Changed => this.telegram_settings_changed(&project, cx),
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.telegram.dialog = Some(OpenDialog {
            view,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_telegram_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.telegram.dialog.take().is_some() {
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    pub(super) fn open_telegram_topic(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        self.close_preview(cx);
        let Some(bridge) = self.telegram.bridge.clone() else {
            self.error = Some("Telegram is not available yet.".into());
            cx.notify();
            return;
        };
        let Some(info) = self.telegram_info(id, cx) else {
            return;
        };
        let title = self.session_metadata(id, cx).display_title();
        let project = project_key(&info.cwd);
        let view = cx.new(|cx| LinkTopicDialog::new(project, info.id, title, bridge, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            |this, _, _: &telegram_dialog::TopicEvent, window, cx| {
                this.close_telegram_topic(window, cx)
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.telegram.topic = Some(OpenTopic {
            view,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_telegram_topic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.telegram.topic.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// A dialog card centred over the window, like the other modals.
    fn telegram_modal_frame(
        &self,
        id: &'static str,
        view: AnyView,
        on_dismiss: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let width = 420f32.min(viewport.width.as_f32() - 24.);
        let weak = cx.weak_entity();
        div()
            .absolute()
            .size_full()
            .child(crate::modal::scrim(SharedString::from(format!(
                "{id}-scrim"
            ))))
            .child(
                popover(id, Anchor::Center)
                    .modal_surface(self.workspace.boxy_layout())
                    .width(width)
                    .gap(0.)
                    .max_height(viewport.height.as_f32() - 32.)
                    .priority(60)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| on_dismiss(this, window, cx));
                    })
                    .child(view),
            )
            .into_any_element()
    }

    pub(super) fn telegram_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.telegram.dialog.as_ref()?.view.clone();
        Some(self.telegram_modal_frame(
            "telegram-project-modal",
            view.into(),
            |this, window, cx| this.close_telegram_dialog(window, cx),
            window,
            cx,
        ))
    }

    pub(super) fn telegram_topic_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.telegram.topic.as_ref()?.view.clone();
        Some(self.telegram_modal_frame(
            "telegram-topic-modal",
            view.into(),
            |this, window, cx| this.close_telegram_topic(window, cx),
            window,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tests::window;
    use gpui::{TestAppContext, WindowHandle};
    use monocode_core::{
        agent::{Decision, Question, QuestionItem, QuestionOption},
        session_groups::WorkItem,
        telegram::forward::{project_key, request_id_of},
    };
    use serde_json::json;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    const PROJECT: &str = "/work/monocode";

    #[derive(Default)]
    struct Recorder {
        statuses: Mutex<HashMap<String, TelegramStatus>>,
        reads: Mutex<usize>,
        forwards: Mutex<Vec<ForwardPayload>>,
        refuse_forwards: Mutex<bool>,
        closed: Mutex<Vec<(String, u64, String)>>,
        results: Mutex<Vec<(String, bool)>>,
    }
    impl Wire for Recorder {
        fn status(&self, project: &str) -> TelegramStatus {
            *self.reads.lock().unwrap() += 1;
            self.statuses
                .lock()
                .unwrap()
                .get(project)
                .cloned()
                .unwrap_or_default()
        }
        fn forward(&self, payload: ForwardPayload) -> Result<(), String> {
            self.forwards.lock().unwrap().push(payload);
            if *self.refuse_forwards.lock().unwrap() {
                Err("Telegram said no".into())
            } else {
                Ok(())
            }
        }
        fn closed(&self, session_id: &str, request_id: u64, status: &str) {
            self.closed
                .lock()
                .unwrap()
                .push((session_id.into(), request_id, status.into()));
        }
        fn answered(&self, key: &str, ok: bool) {
            self.results.lock().unwrap().push((key.into(), ok));
        }
    }
    impl Recorder {
        fn forwards(&self) -> Vec<ForwardPayload> {
            self.forwards.lock().unwrap().clone()
        }
        fn closed(&self) -> Vec<(String, u64, String)> {
            self.closed.lock().unwrap().clone()
        }
        fn results(&self) -> Vec<(String, bool)> {
            self.results.lock().unwrap().clone()
        }
        fn reads(&self) -> usize {
            *self.reads.lock().unwrap()
        }
    }

    fn connected(delay_secs: u64) -> TelegramStatus {
        TelegramStatus {
            configured: true,
            enabled: true,
            bot_username: "alpha_bot".into(),
            chat_id: Some(-100),
            delay_secs,
            ..Default::default()
        }
    }

    fn option(label: &str) -> QuestionOption {
        QuestionOption {
            id: label.to_lowercase(),
            label: label.into(),
            description: String::new(),
        }
    }

    fn question(id: &str) -> Question {
        Question {
            id: id.into(),
            title: "A question".into(),
            auto_resolve_at: None,
            items: vec![
                QuestionItem {
                    id: "one".into(),
                    prompt: "Choose one".into(),
                    multiple: false,
                    allow_custom: true,
                    options: vec![option("First"), option("Second")],
                },
                QuestionItem {
                    id: "many".into(),
                    prompt: "Choose several".into(),
                    multiple: true,
                    allow_custom: false,
                    options: vec![option("A"), option("B")],
                },
            ],
        }
    }

    type Controls = std::sync::mpsc::Receiver<monocode_core::agent::Control>;

    /// A shell whose Telegram wire is a recorder, and its active session.
    fn armed(
        cx: &mut TestAppContext,
        status: TelegramStatus,
    ) -> (WindowHandle<Shell>, i64, Arc<Recorder>, Controls) {
        let (window, _, _) = window(cx);
        let wire = Arc::new(Recorder::default());
        wire.statuses
            .lock()
            .unwrap()
            .insert(project_key(PROJECT), status);
        let (id, controls) = window
            .update(cx, |shell, _, cx| {
                let id = shell.workspace.active.unwrap();
                let controls = shell.sessions[&id].update(cx, |chat, _| chat.listen_for_controls());
                shell.telegram_ready(wire.clone(), None, cx);
                (id, controls)
            })
            .unwrap();
        cx.run_until_parked();
        (window, id, wire, controls)
    }

    fn ask(window: &WindowHandle<Shell>, cx: &mut TestAppContext, id: i64, name: &str) {
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id].update(cx, |chat, cx| chat.push_question(question(name), cx))
            })
            .unwrap();
        cx.run_until_parked();
    }

    fn answer_event(id: i64, question: &str, reply: serde_json::Value) -> serde_json::Value {
        let request = request_id_of(question);
        json!({
            "key": format!("{id}#{request}"),
            "sessionId": id.to_string(),
            "requestId": request,
            "reply": reply,
            "by": "Alice",
        })
    }

    fn picked() -> serde_json::Value {
        json!({"kind": "answered", "answers": {"one": ["second"], "many": ["a", "b"]}, "custom": {}})
    }

    #[gpui::test]
    fn an_unanswered_question_is_forwarded_once(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(0));
        ask(&window, cx, id, "q1");
        let sent = wire.forwards();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].session_id, id.to_string());
        assert_eq!(sent[0].project, project_key(PROJECT));
        assert_eq!(sent[0].request_id, request_id_of("q1"));
        assert_eq!(sent[0].questions.len(), 2);
        window
            .update(cx, |shell, _, cx| shell.telegram_poke(cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(wire.forwards().len(), 1, "a second pass must not resend");
    }

    #[gpui::test]
    fn a_question_waits_for_the_delay_and_the_status_is_read_once(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(300));
        let before = wire.reads();
        ask(&window, cx, id, "q1");
        for _ in 0..3 {
            window
                .update(cx, |shell, _, cx| shell.telegram_poke(cx))
                .unwrap();
            cx.run_until_parked();
        }
        assert!(wire.forwards().is_empty(), "five minutes have not passed");
        assert_eq!(wire.reads() - before, 1, "the cache serves later passes");
    }

    #[gpui::test]
    fn a_changed_setting_is_read_again_once_the_cache_is_invalidated(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(300));
        ask(&window, cx, id, "q1");
        assert!(wire.forwards().is_empty());
        wire.statuses
            .lock()
            .unwrap()
            .insert(project_key(PROJECT), connected(0));
        window
            .update(cx, |shell, _, cx| {
                shell.telegram_settings_changed(PROJECT, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            wire.forwards().len(),
            1,
            "the shorter delay applies at once"
        );
    }

    #[gpui::test]
    fn a_question_is_never_sent_when_forwarding_is_off(cx: &mut TestAppContext) {
        let off = TelegramStatus {
            enabled: false,
            ..connected(0)
        };
        let (window, id, wire, _controls) = armed(cx, off);
        ask(&window, cx, id, "q1");
        wire.statuses
            .lock()
            .unwrap()
            .insert(project_key(PROJECT), connected(0));
        window
            .update(cx, |shell, _, cx| {
                shell.telegram_settings_changed(PROJECT, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert!(wire.forwards().is_empty(), "decided when first seen");
    }

    #[gpui::test]
    fn a_refused_forward_is_tried_twice_and_then_given_up(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(0));
        *wire.refuse_forwards.lock().unwrap() = true;
        ask(&window, cx, id, "q1");
        for _ in 0..4 {
            window
                .update(cx, |shell, _, cx| shell.telegram_poke(cx))
                .unwrap();
            cx.run_until_parked();
        }
        assert_eq!(wire.forwards().len(), 2);
    }

    #[gpui::test]
    fn a_remote_session_is_never_forwarded(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(0));
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id].update(cx, |chat, _| {
                    chat.summary.cwd = "remote://box/work/monocode".into()
                })
            })
            .unwrap();
        ask(&window, cx, id, "q1");
        assert!(wire.forwards().is_empty());
    }

    #[gpui::test]
    fn an_answer_from_telegram_answers_the_question_once(cx: &mut TestAppContext) {
        let (window, id, wire, controls) = armed(cx, connected(0));
        ask(&window, cx, id, "q1");
        let event = answer_event(id, "q1", picked());
        window
            .update(cx, |shell, _, cx| shell.telegram_answer(event.clone(), cx))
            .unwrap();
        cx.run_until_parked();
        let control = controls.try_recv().unwrap();
        assert_eq!(control.id, "q1");
        let Decision::Answer(labels) = control.decision else {
            panic!("expected an answer");
        };
        assert_eq!(
            labels,
            [vec!["Second".to_string()], vec!["A".into(), "B".into()]]
        );
        let key = event["key"].as_str().unwrap().to_string();
        assert_eq!(wire.results(), [(key.clone(), true)]);
        // The question was answered from Telegram, so Telegram is not told it closed.
        window
            .update(cx, |shell, _, cx| shell.telegram_poke(cx))
            .unwrap();
        cx.run_until_parked();
        assert!(wire.closed().is_empty());
        // A replay finds nothing waiting.
        window
            .update(cx, |shell, _, cx| shell.telegram_answer(event.clone(), cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(wire.results(), [(key.clone(), true), (key, false)]);
        assert!(controls.try_recv().is_err());
    }

    #[gpui::test]
    fn two_answers_in_one_update_answer_the_question_only_once(cx: &mut TestAppContext) {
        let (window, id, wire, controls) = armed(cx, connected(0));
        ask(&window, cx, id, "q1");
        let event = answer_event(id, "q1", picked());
        window
            .update(cx, |shell, _, cx| {
                shell.telegram_answer(event.clone(), cx);
                shell.telegram_answer(event.clone(), cx);
            })
            .unwrap();
        cx.run_until_parked();
        let key = event["key"].as_str().unwrap().to_string();
        assert_eq!(wire.results(), [(key.clone(), true), (key, false)]);
        assert!(controls.try_recv().is_ok());
        assert!(controls.try_recv().is_err());
    }

    #[gpui::test]
    fn a_skip_from_telegram_denies_the_question(cx: &mut TestAppContext) {
        let (window, id, wire, controls) = armed(cx, connected(0));
        ask(&window, cx, id, "q1");
        let event = answer_event(id, "q1", json!({"kind": "skipped"}));
        window
            .update(cx, |shell, _, cx| shell.telegram_answer(event, cx))
            .unwrap();
        cx.run_until_parked();
        assert!(matches!(
            controls.try_recv().unwrap().decision,
            Decision::Deny
        ));
        assert_eq!(wire.results().len(), 1);
        assert!(wire.results()[0].1);
    }

    #[gpui::test]
    fn an_answer_for_another_question_or_session_fails(cx: &mut TestAppContext) {
        let (window, id, wire, controls) = armed(cx, connected(0));
        ask(&window, cx, id, "q1");
        let other = answer_event(id, "other", picked());
        let unknown = {
            let mut event = answer_event(id, "q1", picked());
            event["sessionId"] = json!("999999");
            event
        };
        window
            .update(cx, |shell, _, cx| {
                shell.telegram_answer(other.clone(), cx);
                shell.telegram_answer(unknown.clone(), cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(controls.try_recv().is_err());
        let results = wire.results();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|(_, ok)| !ok));
    }

    #[gpui::test]
    fn an_answer_event_without_a_bridge_is_ignored(cx: &mut TestAppContext) {
        let (window, _, id) = window(cx);
        let event = answer_event(id, "q1", picked());
        window
            .update(cx, |shell, _, cx| {
                shell.telegram_answer(event, cx);
                shell.telegram_poke(cx);
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn answering_on_the_desktop_closes_the_telegram_message(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(0));
        ask(&window, cx, id, "q1");
        let answers = vec![vec!["First".to_string()], vec!["A".to_string()]];
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id]
                    .update(cx, |chat, cx| chat.answer_question("q1", Some(answers), cx))
                    .unwrap()
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            wire.closed(),
            [(
                id.to_string(),
                request_id_of("q1"),
                "Answered on desktop".to_string()
            )]
        );
    }

    #[gpui::test]
    fn skipping_on_the_desktop_says_skipped(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(0));
        ask(&window, cx, id, "q1");
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id]
                    .update(cx, |chat, cx| chat.answer_question("q1", None, cx))
                    .unwrap()
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(wire.closed()[0].2, "Skipped");
    }

    #[gpui::test]
    fn a_stopped_turn_says_cancelled(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(0));
        ask(&window, cx, id, "q1");
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id].update(cx, |chat, cx| chat.cancel_questions(cx))
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(wire.closed()[0].2, "Cancelled");
    }

    #[gpui::test]
    fn a_question_answered_before_it_was_sent_is_never_sent(cx: &mut TestAppContext) {
        let (window, id, wire, _controls) = armed(cx, connected(300));
        ask(&window, cx, id, "q1");
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id].update(cx, |chat, cx| chat.answer_question("q1", None, cx))
            })
            .unwrap()
            .unwrap();
        cx.run_until_parked();
        assert!(wire.forwards().is_empty());
        assert!(wire.closed().is_empty());
    }

    fn info(
        window: &WindowHandle<Shell>,
        cx: &mut TestAppContext,
        id: i64,
    ) -> monocode_core::telegram::forward::SessionInfo {
        window
            .update(cx, |shell, _, cx| shell.telegram_info(id, cx).unwrap())
            .unwrap()
    }

    fn work_item(kind: &str, number: u64) -> Option<WorkItem> {
        Some(WorkItem {
            kind: kind.into(),
            repo: "o/r".into(),
            number,
            url: format!("https://example.com/{number}"),
        })
    }

    #[gpui::test]
    fn only_a_github_work_item_gives_a_session_a_number(cx: &mut TestAppContext) {
        let (window, id, _, _) = armed(cx, connected(0));
        let set = |window: &WindowHandle<Shell>, cx: &mut TestAppContext, item| {
            window
                .update(cx, |shell, _, cx| {
                    shell.sessions[&id].update(cx, |chat, _| chat.summary.work_item = item)
                })
                .unwrap()
        };
        assert_eq!(info(&window, cx, id).linked_number, None);
        for (kind, expected) in [
            ("issue", Some(42)),
            ("pr", Some(42)),
            ("clickup", None),
            ("linear", None),
            ("jira", None),
        ] {
            set(&window, cx, work_item(kind, 42));
            assert_eq!(info(&window, cx, id).linked_number, expected, "{kind}");
        }
    }

    #[gpui::test]
    fn the_inbox_ticket_names_the_session(cx: &mut TestAppContext) {
        let (window, id, _, _) = armed(cx, connected(0));
        assert_eq!(info(&window, cx, id).ticket, None);
        window
            .update(cx, |shell, _, _| {
                shell.session_tickets.insert(id, "ENG-42".into());
            })
            .unwrap();
        let info = info(&window, cx, id);
        assert_eq!(info.ticket.as_deref(), Some("ENG-42"));
        assert_eq!(info.id, id.to_string());
        assert_eq!(info.cwd, PROJECT);
        assert!(!info.mono && !info.orchestration_worker);
    }

    #[gpui::test]
    fn a_pending_question_brings_the_last_message_into_the_snapshot(cx: &mut TestAppContext) {
        let (window, id, _, _) = armed(cx, connected(300));
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id].update(cx, |chat, cx| {
                    chat.transcript.update(cx, |transcript, cx| {
                        transcript.patch(
                            0,
                            vec![monocode_core::transcript::Row {
                                role: "assistant".into(),
                                text: "I can do either. Which one?".into(),
                                first: true,
                                sent_at: None,
                            }],
                            cx,
                        )
                    })
                })
            })
            .unwrap();
        ask(&window, cx, id, "q1");
        let snapshots = window
            .update(cx, |shell, _, cx| shell.telegram_snapshots(cx))
            .unwrap();
        assert_eq!(snapshots.infos.len(), 1);
        let session = &snapshots.infos[0];
        assert_eq!(session.pending.as_ref().unwrap().id, "q1");
        assert_eq!(session.blocks[0].text, "I can do either. Which one?");
        assert_eq!(snapshots.ids[&id.to_string()], id);
    }

    #[test]
    fn the_sink_hands_answers_to_the_ui_without_blocking() {
        let (sender, receiver) = async_channel::unbounded();
        let sink = AppSink { sender };
        let payload = json!({"key": "k"});
        assert!(sink.emit_to(WINDOW, ANSWER_EVENT, payload.clone()));
        assert_eq!(receiver.try_recv().unwrap(), payload);
        // Another window or event is not ours.
        assert!(!sink.emit_to("other", ANSWER_EVENT, payload.clone()));
        assert!(!sink.emit_to(WINDOW, "something-else", payload.clone()));
        assert!(receiver.try_recv().is_err());
        // Without a UI loop to drain it the answer counts as failed.
        drop(receiver);
        assert!(!sink.emit_to(WINDOW, ANSWER_EVENT, payload));
    }

    #[gpui::test]
    fn the_project_menu_offers_telegram_except_for_remote_projects(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                let ids = |shell: &Shell, path: &str, w: &mut Window, cx: &mut Context<Shell>| {
                    let target = MenuTarget::Project {
                        path: path.into(),
                        rail: false,
                    };
                    let (_, items, _, _) = shell.menu_items(&target, &mut Vec::new(), w, cx);
                    items
                        .into_iter()
                        .filter_map(|item| match item {
                            MenuItem::Action(action) => Some((action.id, action.label)),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                };
                let local = ids(shell, PROJECT, w, cx);
                let at = local.iter().position(|(id, _)| id == "telegram").unwrap();
                assert_eq!(local[at].1, "Telegram…");
                assert_eq!(
                    local[at - 1].0,
                    "background",
                    "right after the background item"
                );
                let remote = ids(shell, "remote://box/work/monocode", w, cx);
                assert!(remote.iter().all(|(id, _)| id != "telegram"));
            })
            .unwrap();
    }

    fn session_items(shell: &Shell, id: i64, cx: &App) -> Vec<String> {
        shell
            .session_menu_items(id, cx)
            .into_iter()
            .filter_map(|item| match item {
                MenuItem::Action(action) => Some(action.id.to_string()),
                _ => None,
            })
            .collect()
    }

    #[gpui::test]
    fn the_session_menu_links_a_topic_only_for_a_project_with_a_group(cx: &mut TestAppContext) {
        let (window, id, _, _) = armed(cx, connected(300));
        window
            .update(cx, |shell, _, cx| {
                let at = |shell: &Shell, cx: &App| {
                    session_items(shell, id, cx)
                        .iter()
                        .position(|item| item == "link-telegram-topic")
                };
                // Nothing is known until the project's status was read.
                shell.telegram.linkable.clear();
                assert_eq!(at(shell, cx), None);
                shell.telegram.linkable.insert(project_key(PROJECT), true);
                let items = session_items(shell, id, cx);
                let link = at(shell, cx).unwrap();
                assert_eq!(items[link - 1], "link-work-item");
                shell.telegram.linkable.insert(project_key(PROJECT), false);
                assert_eq!(at(shell, cx), None);
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_status_read_decides_whether_a_project_is_linkable(cx: &mut TestAppContext) {
        let (window, _, wire, _) = armed(cx, connected(300));
        wire.statuses.lock().unwrap().insert(
            "/work/another-project".into(),
            TelegramStatus {
                chat_id: None,
                ..connected(300)
            },
        );
        window
            .update(cx, |shell, _, cx| shell.telegram_refresh_linkable(cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.telegram.linkable.get(&project_key(PROJECT)),
                    Some(&true)
                );
                assert_eq!(
                    shell
                        .telegram
                        .linkable
                        .get(&project_key("/work/another-project")),
                    Some(&false)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_session_menu_hides_the_link_for_a_remote_session(cx: &mut TestAppContext) {
        let (window, id, _, _) = armed(cx, connected(300));
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id].update(cx, |chat, _| {
                    chat.summary.cwd = "remote://box/work/monocode".into()
                });
                shell
                    .telegram
                    .linkable
                    .insert(project_key("remote://box/work/monocode"), true);
                assert!(!session_items(shell, id, cx).contains(&"link-telegram-topic".to_string()));
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_menu_items_open_and_close_the_dialogs(cx: &mut TestAppContext) {
        let (window, id, _, _) = armed(cx, connected(300));
        let dir = std::env::temp_dir().join(format!(
            "monoptah-telegram-shell-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (sender, _receiver) = async_channel::unbounded();
        let bridge = telegram::open("http://127.0.0.1:1", &dir, Arc::new(AppSink { sender }));
        window
            .update(cx, |shell, _, _| shell.telegram.bridge = Some(bridge))
            .unwrap();
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(
                    MenuTarget::Project {
                        path: PROJECT.into(),
                        rail: false,
                    },
                    "telegram",
                    None,
                    w,
                    cx,
                );
                assert!(shell.telegram.dialog.is_some());
                shell.close_telegram_dialog(w, cx);
                assert!(shell.telegram.dialog.is_none());
                shell.run_menu(MenuTarget::Session(id), "link-telegram-topic", None, w, cx);
                assert!(shell.telegram.topic.is_some());
                shell.close_telegram_topic(w, cx);
                assert!(shell.telegram.topic.is_none());
            })
            .unwrap();
        std::fs::remove_dir_all(dir).ok();
    }

    #[gpui::test]
    fn the_dialogs_draw_in_the_shell_and_escape_closes_them(cx: &mut TestAppContext) {
        let (window, id, _, _) = armed(cx, connected(300));
        cx.update(crate::input::bind);
        let dir = std::env::temp_dir().join(format!(
            "monoptah-telegram-shell-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (sender, _receiver) = async_channel::unbounded();
        let bridge = telegram::open("http://127.0.0.1:1", &dir, Arc::new(AppSink { sender }));
        window
            .update(cx, |shell, w, cx| {
                shell.telegram.bridge = Some(bridge);
                shell.open_telegram_dialog(PROJECT.into(), w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(visual.debug_bounds("telegram-dialog").is_some());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                assert!(shell.telegram.dialog.is_none());
                shell.open_telegram_topic(id, w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(visual.debug_bounds("telegram-topic-dialog").is_some());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        window
            .update(cx, |shell, _, _| assert!(shell.telegram.topic.is_none()))
            .unwrap();
        std::fs::remove_dir_all(dir).ok();
    }
}
