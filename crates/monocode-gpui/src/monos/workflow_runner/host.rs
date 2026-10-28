//! The app side of a workflow run: sessions started through the `sessions.start` path, follow-ups,
//! hidden Ask-Mono sessions, the attention shelf and the report in the Mono's chat.
use super::sync::race;
use super::{
    ApprovalAsk, Fut, Outcome, Posted, STOPPED_BY_USER, SessionInfo, SessionRequest,
    StartedSession, WorkflowHost,
};
use crate::conversation::{TurnEnd, WorkflowTurn};
use crate::monos::host::{Effect, StartContext, StartOptions};
use crate::monos::*;
use async_channel::{Receiver, Sender, bounded};
use gpui::AsyncApp;
use monocode_core::{
    mono::workflows::{
        command::{self, CommandResult},
        model::{Checkout, Node, NodeKind, Workflow, WorkflowRun},
        models::{ModelCatalog, Probe, harnesses_used, session_node_problem},
        ordered::OrderedMap,
        store::{MonoDir, update_runs},
    },
    store::native_model,
};
use serde_json::json;
use std::{rc::Rc, time::Duration};

/// The hidden session of an Ask-Mono question gives up a little before the run does, so its
/// clean-up still happens.
const ASK_SESSION_TIMEOUT: Duration = Duration::from_secs(10 * 60 - 5);
/// How often a watched session is looked at besides when its conversation changes.
const WATCH_POLL: Duration = Duration::from_secs(1);
/// A session that never shows a turn going is not waited for forever.
const UNSTARTED_POLLS: u32 = 10;
const CHAT_UNAVAILABLE: &str = "The Mono's chat is not available";
/// How many 200 ms looks a run waits for a Mono's chat to load (a minute): a run requested before
/// a quit starts at the first look of the next start, while chats are still loading.
const CHAT_WAIT_ATTEMPTS: u32 = 300;

/// A probe that finds every harness installed and lists no models, so a harness is never
/// refused; what a test wants refused it says itself.
#[cfg(test)]
fn trusting_probe(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: None,
    }
}

/// An Ask-Mono question being answered by its hidden session.
pub(in crate::monos) struct Ask {
    mono: String,
    pub(in crate::monos) view: Entity<Conversation>,
    reply: Sender<Result<String, String>>,
    dispatched: bool,
    /// Ends with the question.
    _changes: Subscription,
}

/// What the app keeps for its workflow runs next to the runner.
pub(in crate::monos) struct Live {
    /// Wake-ups for a watched session: its conversation changed.
    wakers: HashMap<i64, (Sender<()>, Receiver<()>)>,
    /// Ends when the run lets the session go.
    watches: HashMap<i64, Subscription>,
    /// Sessions waiting on the user, by Mono, for the attention shelf.
    attention: HashMap<String, HashSet<i64>>,
    pub(in crate::monos) asks: HashMap<i64, Ask>,
    /// Persisted session ids (what run files hold) to local ones.
    ids: HashMap<String, i64>,
    probe: fn(Kind) -> Probe,
}

impl Live {
    pub(in crate::monos) fn new() -> Self {
        Self {
            wakers: HashMap::new(),
            watches: HashMap::new(),
            attention: HashMap::new(),
            asks: HashMap::new(),
            ids: HashMap::new(),
            #[cfg(test)]
            probe: trusting_probe,
            #[cfg(not(test))]
            probe: monocode_core::mono::workflows::models::native_probe,
        }
    }

    /// What is still kept for sessions and questions.
    #[cfg(test)]
    pub(in crate::monos) fn kept(&self) -> usize {
        self.watches.len() + self.wakers.len() + self.ids.len() + self.asks.len()
    }

    /// How a harness is probed when a workflow's models are checked.
    pub(in crate::monos) fn probe(&self) -> fn(Kind) -> Probe {
        self.probe
    }

    /// The sessions of `mono`'s runs that wait on the user.
    pub(in crate::monos) fn attention(&self, mono: &str) -> Vec<i64> {
        self.attention
            .get(mono)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The Mono whose hidden question session this is.
    pub(in crate::monos) fn asked_by(&self, session: i64) -> Option<&str> {
        self.asks.get(&session).map(|ask| ask.mono.as_str())
    }

    fn wake(&mut self, session: i64) -> &(Sender<()>, Receiver<()>) {
        self.wakers.entry(session).or_insert_with(|| bounded(1))
    }

    #[cfg(test)]
    pub(in crate::monos) fn with_probe(mut self, probe: fn(Kind) -> Probe) -> Self {
        self.probe = probe;
        self
    }
}

/// How a conversation's turn maps onto the outcome a run reads.
pub(crate) fn outcome_of(turn: WorkflowTurn) -> Outcome {
    match turn.end {
        TurnEnd::Completed => Outcome::completed(turn.reply),
        TurnEnd::Cancelled => Outcome {
            text: turn.reply,
            ..Outcome::cancelled(STOPPED_BY_USER)
        },
        TurnEnd::Failed(status) => Outcome {
            text: turn.reply,
            ..Outcome::failed(status)
        },
    }
}

/// The harness a stored model id names (`grok` of `grok:grok-4.6`).
fn model_harness(model: &str) -> Option<&str> {
    let (harness, rest) = model.split_once(':')?;
    (Kind::parse(harness).is_some() && !rest.is_empty()).then_some(harness)
}

pub(super) enum Seen {
    Working,
    Over(WorkflowTurn),
}

/// What one look at a watched session means: its outcome, or `None` to keep waiting. `armed`
/// says a turn has been seen going; an idle session that never was is given up on after a while,
/// unless it ended in an error (a prompt that failed to start says why).
pub(super) fn look(seen: Seen, armed: &mut bool, unstarted: &mut u32) -> Option<Outcome> {
    match seen {
        Seen::Working => {
            *armed = true;
            None
        }
        Seen::Over(turn) if *armed || matches!(turn.end, TurnEnd::Failed(_)) => {
            Some(outcome_of(turn))
        }
        Seen::Over(_) => {
            *unstarted += 1;
            (*unstarted > UNSTARTED_POLLS)
                .then(|| Outcome::failed("The session did not start a turn."))
        }
    }
}

/// Waits for the turn a session is in to end. Its conversation wakes this; the poll is a net.
async fn settle(
    monos: WeakEntity<Monos>,
    mut cx: AsyncApp,
    session: i64,
    wake: Receiver<()>,
    mut armed: bool,
) -> Outcome {
    let mut unstarted = 0;
    loop {
        match monos.update(&mut cx, |m, cx| m.workflow_turn(session, cx)) {
            Err(_) => return Outcome::failed("The app is closing."),
            Ok(None) => return Outcome::failed("The session is no longer available."),
            Ok(Some(seen)) => {
                if let Some(outcome) = look(seen, &mut armed, &mut unstarted) {
                    return outcome;
                }
            }
        }
        let _ = race(wake.recv(), cx.background_executor().timer(WATCH_POLL)).await;
    }
}

impl Monos {
    fn workflow_local_id(&mut self, persisted: &str) -> Option<i64> {
        if let Some(id) = self.workflow_live.ids.get(persisted) {
            return Some(*id);
        }
        let id = locked(&self.store)
            .ok()?
            .resolve_session_id(persisted)
            .ok()?;
        self.workflow_live.ids.insert(persisted.into(), id);
        Some(id)
    }

    fn workflow_view(
        &mut self,
        persisted: &str,
        cx: &mut Context<Self>,
    ) -> Option<(i64, Entity<Conversation>)> {
        let id = self.workflow_local_id(persisted)?;
        let view = self
            .owner
            .upgrade()?
            .update(cx, |owner, cx| owner.mono_ensure_session(id, cx))?;
        Some((id, view))
    }

    /// The conversations of `mono`'s workflow runs that wait on the user, for the attention shelf.
    pub(in crate::monos) fn workflow_attention_views(
        &self,
        mono: &str,
        cx: &App,
    ) -> Vec<Entity<Conversation>> {
        let Some(owner) = self.owner.upgrade() else {
            return Vec::new();
        };
        self.workflow_live
            .attention(mono)
            .into_iter()
            .filter_map(|session| owner.read(cx).mono_session_entity(session))
            .filter(|view| view.read(cx).needs_approval())
            .collect()
    }

    /// Wakes whoever waits on the session when its conversation changes.
    fn workflow_watch(
        &mut self,
        session: i64,
        view: &Entity<Conversation>,
        cx: &mut Context<Self>,
    ) {
        if self.workflow_live.watches.contains_key(&session) {
            return;
        }
        let wake = self.workflow_live.wake(session).0.clone();
        let watch = cx.subscribe(view, move |_, _, _: &SessionChanged, _| {
            let _ = wake.try_send(());
        });
        self.workflow_live.watches.insert(session, watch);
    }

    /// A run is done with a session: its watch, wake-up and cached id go.
    pub(in crate::monos) fn workflow_release(&mut self, persisted: &str) {
        if let Some(local) = self.workflow_live.ids.remove(persisted) {
            self.workflow_live.watches.remove(&local);
            self.workflow_live.wakers.remove(&local);
        }
    }

    pub(in crate::monos) fn workflow_settled(
        &mut self,
        session: i64,
        armed: bool,
        cx: &mut Context<Self>,
    ) -> Fut<Outcome> {
        let wake = self.workflow_live.wake(session).1.clone();
        Box::pin(settle(
            cx.weak_entity(),
            cx.to_async(),
            session,
            wake,
            armed,
        ))
    }

    fn workflow_turn(&mut self, session: i64, cx: &mut Context<Self>) -> Option<Seen> {
        let view = self
            .owner
            .upgrade()?
            .update(cx, |owner, cx| owner.mono_ensure_session(session, cx))?;
        let view = view.read(cx);
        Some(match view.workflow_turn(cx) {
            Some(turn) => Seen::Over(turn),
            None => Seen::Working,
        })
    }

    /// Starts a workflow node's session through the `sessions.start` path as the Mono: full
    /// access, no completion report, listed in the sidebar as started by the Mono. Everything
    /// that can be refused is refused before the session exists. `Ok(false)`: the Mono's chat is
    /// still loading, ask again. `draft` only puts the prompt in the composer (tests).
    pub(in crate::monos) fn workflow_session(
        &mut self,
        mono_id: &str,
        req: SessionRequest,
        draft: bool,
        reply: Sender<Result<StartedSession, String>>,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let Some(mono) = self.roster.get(mono_id).cloned() else {
            return Err(CHAT_UNAVAILABLE.into());
        };
        self.ensure_chat(mono_id, cx);
        let Some(chat) = self.chats.get(mono_id).cloned() else {
            return Ok(false);
        };
        if !mono.works_on(&req.project) {
            return Err("Not one of your projects".into());
        }
        // A model alone is started in its own harness, as the picker pins both.
        let harness = req.harness.clone().or_else(|| {
            req.model
                .as_deref()
                .and_then(model_harness)
                .map(str::to_owned)
        });
        let mut input = json!({
            "project": req.project,
            "prompt": req.prompt,
            "runtimeMode": "full-access",
            "notifyOnComplete": false,
            "draft": draft,
        });
        if let Some(harness) = &harness {
            input["harness"] = json!(harness);
        }
        if let Some(model) = &req.model {
            // Nodes hold `<harness>:<name>`; the conversation holds the id the CLI is started with.
            let native = harness.as_deref().and_then(|h| native_model(model, h));
            if let Some(native) = native.or_else(|| harness.is_none().then(|| model.clone())) {
                input["model"] = json!(native);
            }
        }
        if let Some(effort) = &req.effort {
            input["effort"] = json!(effort);
        }
        if req.new_worktree {
            input["workspaceMode"] = json!("worktree");
        }
        if let Some(cwd) = &req.worktree_cwd {
            input["worktreeCwd"] = json!(cwd);
        }
        // Nesting is decoration: a parent the app does not know is just a top-level session.
        if let Some(parent) = req
            .parent_session_id
            .as_deref()
            .and_then(|parent| self.workflow_local_id(parent))
        {
            input["parentSessionId"] = json!(parent);
        }
        if let Some(folder) = &req.folder_id {
            input["folderId"] = json!(folder);
        }
        if let Some(name) = &req.folder_name {
            input["folderName"] = json!(name);
        }
        let options = StartOptions {
            lenient_parent: true,
            visible: true,
            catalog_checked: true,
        };
        let context = StartContext {
            request: mono::new_id(),
            placement: None,
            options,
        };
        let work =
            self.prepare_start(&mono, req.project.clone(), Some(chat), &input, context, cx)?;
        // The catalog check may start a CLI, so it runs with the rest off the UI thread.
        let check = (harness.is_some() || req.model.is_some() || req.effort.is_some()).then(|| {
            Node::new(
                "start",
                NodeKind::Session {
                    project: String::new(),
                    prompt: String::new(),
                    checkout: Checkout::Root,
                    output: OrderedMap::new(),
                    harness: harness.clone(),
                    model: req.model.clone(),
                    effort: req.effort.clone(),
                    reuse_session: None,
                    nest_under: None,
                    timeout_minutes: None,
                },
            )
        });
        let probe = self.workflow_live.probe;
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            if let Some(node) = check {
                let mut probed = Workflow::new("wf-start", "start");
                probed.nodes.push(node.clone());
                let catalog = ModelCatalog::for_harnesses(&harnesses_used(&probed), probe);
                if let Some(problem) = session_node_problem(&node, &catalog) {
                    return Err(problem);
                }
            }
            let (value, effect) = work(store.clone())?;
            let id = value["sessionId"]
                .as_i64()
                .ok_or("The session was not created")?;
            let persisted = locked(&store)?.persisted_session_id(id)?;
            Ok((value, effect, persisted))
        });
        let mono_id = mono_id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let started = result.and_then(|(value, effect, persisted)| match effect {
                    Effect::Start(started) => Ok((value, *started, persisted)),
                    _ => Err("The session was not created".to_string()),
                });
                let _ = reply.try_send(match started {
                    Err(error) => Err(error),
                    Ok((value, started, persisted)) => {
                        let local = started.summary.id;
                        let caller = this
                            .roster
                            .get(&mono_id)
                            .and_then(|m| m.session_id)
                            .unwrap_or(0);
                        this.apply_started(&mono_id, caller, &input, started, None, cx);
                        this.workflow_live.ids.insert(persisted.clone(), local);
                        let view = this
                            .owner
                            .upgrade()
                            .and_then(|owner| owner.read(cx).mono_session_entity(local));
                        let armed = match &view {
                            Some(view) => {
                                this.workflow_watch(local, view, cx);
                                view.read(cx).workflow_working()
                            }
                            None => false,
                        };
                        let settled = this.workflow_settled(local, armed, cx);
                        this.reload_files(&mono_id, cx);
                        this.changed(cx);
                        Ok(StartedSession {
                            session_id: persisted,
                            folder_id: value["folderId"].as_str().map(str::to_owned),
                            settled,
                        })
                    }
                });
            });
        })
        .detach();
        Ok(true)
    }

    /// Sends a follow-up into a session a node started earlier.
    fn workflow_send(
        &mut self,
        persisted: &str,
        prompt: String,
        cx: &mut Context<Self>,
    ) -> Result<Fut<Outcome>, String> {
        let (local, view) = self
            .workflow_view(persisted, cx)
            .ok_or("The session is no longer available.")?;
        view.update(cx, |v, cx| v.mono_external_send(prompt, false, cx))?;
        self.workflow_watch(local, &view, cx);
        let armed = view.read(cx).workflow_working();
        Ok(self.workflow_settled(local, armed, cx))
    }

    fn workflow_stop(&mut self, persisted: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(local) = self.workflow_local_id(persisted) else {
            return Ok(());
        };
        if self.is_mono_session(local) {
            return Err("The session cannot be stopped from a workflow".into());
        }
        if let Some((_, view)) = self.workflow_view(persisted, cx) {
            view.update(cx, |v, cx| v.mono_stop_turn(cx));
        }
        Ok(())
    }

    fn workflow_info(&mut self, persisted: &str, cx: &mut Context<Self>) -> Option<SessionInfo> {
        let (_, view) = self.workflow_view(persisted, cx)?;
        let view = view.read(cx);
        Some(SessionInfo {
            session_id: persisted.into(),
            cwd: view.summary.cwd.clone(),
            title: view.summary.title.clone(),
            harness: view.summary.provider.clone(),
            model: view.mono_model(),
        })
    }

    /// Puts the run's report in the Mono's chat, with a card for each session the run started.
    pub(in crate::monos) fn workflow_post(&mut self, report: Posted, cx: &mut Context<Self>) {
        // A Mono that was deleted has no chat to report to.
        if self.roster.get(&report.mono_id).is_none() {
            return;
        }
        let cards: Vec<String> = report
            .launches
            .iter()
            .filter_map(|launch| {
                let session = self.workflow_local_id(&launch.session_id)?;
                Some(format!(
                    "mono-card:{}",
                    json!({"id": mono::new_id(), "type": "session", "sessionId": session, "title": launch.title, "state": "Saved"})
                ))
            })
            .collect();
        if let Some(mono) = self.roster.get_mut(&report.mono_id) {
            mono.pending_reports.push(report.text);
            mono.pending_reports.extend(cards);
        }
        self.ensure_chat(&report.mono_id, cx);
        self.save(cx);
        self.dispatch(&report.mono_id, cx);
    }

    fn workflow_attention(&mut self, mono: &str, session: &str, on: bool, cx: &mut Context<Self>) {
        let Some(local) = self.workflow_local_id(session) else {
            return;
        };
        let changed = if on {
            self.workflow_live
                .attention
                .entry(mono.into())
                .or_default()
                .insert(local)
        } else {
            self.workflow_live
                .attention
                .values_mut()
                .fold(false, |any, set| set.remove(&local) | any)
        };
        if changed {
            cx.notify();
        }
    }

    /// Asks the Mono a question in a hidden session of its own, like a habit run but without
    /// holding `active_run`: habits keep running. The reply goes to `reply` once the session has
    /// answered; the session is removed after. `Ok(false)`: the Mono's chat is still loading, ask
    /// again.
    fn workflow_ask(
        &mut self,
        mono_id: &str,
        prompt: String,
        reply: Sender<Result<String, String>>,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let Some(mono) = self.roster.get(mono_id).cloned() else {
            return Err(CHAT_UNAVAILABLE.into());
        };
        self.ensure_chat(mono_id, cx);
        let Some(chat) = self.chats.get(mono_id).cloned() else {
            return Ok(false);
        };
        let inherited = chat.read(cx).settings.clone();
        let provider = chat.read(cx).summary.provider.clone();
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let db = locked(&store)?;
            let cwd = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            // Always a chat: the app drives this session, whatever the surface preference.
            let mut summary = db.create(&provider, &cwd)?;
            summary.title = format!("{} \u{b7} workflow question", mono.label());
            db.title(summary.id, &summary.title)?;
            Ok::<_, String>((summary, mono, inherited))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Err(error) => {
                    let _ = reply.try_send(Err(error));
                }
                Ok((summary, mono, settings)) => {
                    this.workflow_ask_start(summary, mono, settings, prompt, reply, cx)
                }
            });
        })
        .detach();
        Ok(true)
    }

    fn workflow_ask_start(
        &mut self,
        summary: monocode_core::store::Session,
        mono: mono::Mono,
        mut settings: Settings,
        prompt: String,
        reply: Sender<Result<String, String>>,
        cx: &mut Context<Self>,
    ) {
        let session = summary.id;
        let view = cx.new(|cx| Conversation::new(summary, self.store.clone(), cx));
        let env = self
            .bridge
            .as_ref()
            .map(|b| b.environment(session))
            .unwrap_or_default();
        settings.access = AccessMode::FullAccess;
        settings.internal_message = true;
        settings.mono = None;
        settings.environment.clear();
        let mono_id = mono.id.clone();
        let root = self.root.clone();
        view.update(cx, |v, cx| {
            v.install_transfer(settings, None, cx);
            v.configure_mono(mono, root, env, cx);
        });
        let changes = cx.subscribe(&view, move |this, _, _: &SessionChanged, cx| {
            this.workflow_ask_changed(session, cx)
        });
        self.roster.hidden_sessions.push(session);
        self.roster.ask_sessions.push(session);
        self.workflow_live.asks.insert(
            session,
            Ask {
                mono: mono_id,
                view: view.clone(),
                reply,
                dispatched: false,
                _changes: changes,
            },
        );
        self.save(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ASK_SESSION_TIMEOUT).await;
            let _ = this.update(cx, |this, cx| {
                this.workflow_ask_end(session, Err("The Mono did not answer in time".into()), cx)
            });
        })
        .detach();
        // A test never starts a provider; it ends the question itself.
        if cx.is_test() {
            return;
        }
        let message = Message {
            sent_at: Some(monocode_core::time::now_ms()),
            id: mono::new_id(),
            text: prompt,
            internal: true,
            error: None,
            attachments: Vec::new(),
        };
        if !view.update(cx, |v, cx| v.mono_dispatch(&message, cx)) {
            return self.workflow_ask_end(
                session,
                Err("The question could not be sent".into()),
                cx,
            );
        }
        if let Some(ask) = self.workflow_live.asks.get_mut(&session) {
            ask.dispatched = true;
        }
        self.workflow_ask_changed(session, cx);
    }

    fn workflow_ask_changed(&mut self, session: i64, cx: &mut Context<Self>) {
        let Some(ask) = self
            .workflow_live
            .asks
            .get(&session)
            .filter(|a| a.dispatched)
        else {
            return;
        };
        let Some(turn) = ask.view.read(cx).workflow_turn(cx) else {
            return;
        };
        let result = match turn.end {
            TurnEnd::Completed => Ok(turn.reply),
            TurnEnd::Cancelled => Err("The Mono did not answer".into()),
            TurnEnd::Failed(status) => Err(status),
        };
        self.workflow_ask_end(session, result, cx);
    }

    /// Hands the answer over and removes the hidden session. Does nothing for one already ended.
    pub(in crate::monos) fn workflow_ask_end(
        &mut self,
        session: i64,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ask) = self.workflow_live.asks.remove(&session) else {
            return;
        };
        let _ = ask.reply.try_send(result);
        let view = ask.view.clone();
        view.update(cx, |v, cx| v.mono_stop(cx));
        // A stopped turn may still be writing its rows: the session goes once it has settled.
        cx.spawn(async move |this, cx| {
            for _ in 0..60 {
                let working = this
                    .update(cx, |_, cx| view.read(cx).workflow_working())
                    .unwrap_or(false);
                if !working {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
            }
            let _ = this.update(cx, |this, cx| this.forget_asks(vec![session], cx));
        })
        .detach();
    }

    /// The real host for a run of `mono_id`'s workflow.
    pub(in crate::monos) fn workflow_host(&self, cx: &mut Context<Self>) -> Rc<dyn WorkflowHost> {
        Rc::new(AppHost {
            monos: cx.weak_entity(),
            cx: cx.to_async(),
        })
    }
}

/// `WorkflowHost` over the app's `Monos`. Its methods are called from the run's tasks, never from
/// inside an update of `Monos`.
pub(crate) struct AppHost {
    monos: WeakEntity<Monos>,
    cx: AsyncApp,
}

impl AppHost {
    fn with<R>(&self, f: impl FnOnce(&mut Monos, &mut Context<Monos>) -> R) -> Option<R> {
        self.monos.update(&mut self.cx.clone(), f).ok()
    }
}

fn closing<T>() -> Result<T, String> {
    Err("The app is closing".into())
}

/// Calls `ask` until it reports the Mono's chat ready: right after the app starts it may still be
/// loading.
async fn once_chat_is_ready(
    monos: &WeakEntity<Monos>,
    cx: &mut AsyncApp,
    mut ask: impl FnMut(&mut Monos, &mut Context<Monos>) -> Result<bool, String>,
) -> Result<(), String> {
    let mut attempts = 0;
    loop {
        match monos.update(cx, |m, cx| ask(m, cx)) {
            Err(_) => return closing(),
            Ok(Err(error)) => return Err(error),
            Ok(Ok(true)) => return Ok(()),
            Ok(Ok(false)) if attempts >= CHAT_WAIT_ATTEMPTS => return Err(CHAT_UNAVAILABLE.into()),
            Ok(Ok(false)) => {
                attempts += 1;
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
            }
        }
    }
}

impl WorkflowHost for AppHost {
    fn start_session(
        &self,
        mono_id: &str,
        req: SessionRequest,
    ) -> Fut<Result<StartedSession, String>> {
        let (monos, mut cx, mono_id) = (self.monos.clone(), self.cx.clone(), mono_id.to_owned());
        Box::pin(async move {
            let (tx, rx) = bounded(1);
            once_chat_is_ready(&monos, &mut cx, |m, cx| {
                m.workflow_session(&mono_id, req.clone(), false, tx.clone(), cx)
            })
            .await?;
            rx.recv().await.unwrap_or_else(|_| closing())
        })
    }

    fn send(&self, session_id: &str, prompt: String) -> Fut<Outcome> {
        let sent = self.with(|m, cx| m.workflow_send(session_id, prompt, cx));
        match sent {
            Some(Ok(settled)) => settled,
            Some(Err(error)) => Box::pin(async move { Outcome::failed(error) }),
            None => Box::pin(async { Outcome::failed("The app is closing") }),
        }
    }

    fn stop(&self, session_id: &str) -> Fut<Result<(), String>> {
        let stopped = self
            .with(|m, cx| m.workflow_stop(session_id, cx))
            .unwrap_or_else(closing);
        Box::pin(async move { stopped })
    }

    fn final_reply(&self, session_id: &str) -> String {
        self.with(|m, cx| {
            m.workflow_view(session_id, cx)
                .map(|(_, view)| view.read(cx).workflow_reply(cx))
        })
        .flatten()
        .unwrap_or_default()
    }

    fn waiting_approval(&self, session_id: &str) -> bool {
        self.with(|m, cx| {
            m.workflow_view(session_id, cx)
                .is_some_and(|(_, view)| view.read(cx).needs_approval())
        })
        .unwrap_or(false)
    }

    fn ask_approval(&self, ask: ApprovalAsk) {
        self.with(|m, cx| m.workflow_attention(&ask.mono_id, &ask.session_id, true, cx));
    }

    fn end_approvals(&self, session_id: &str) {
        self.with(|m, cx| m.workflow_attention("", session_id, false, cx));
    }

    fn release_session(&self, session_id: &str) {
        self.with(|m, _| m.workflow_release(session_id));
    }

    fn session_info(&self, session_id: &str) -> Option<SessionInfo> {
        self.with(|m, cx| m.workflow_info(session_id, cx)).flatten()
    }

    fn run_command(
        &self,
        cwd: String,
        command: String,
        timeout_ms: u64,
        id: String,
    ) -> Fut<Result<CommandResult, String>> {
        // The call blocks until the command ends, possibly for hours. Not on the UI thread, and
        // not on the executor's pool either (fixed at the CPU count on Linux): a few long
        // commands would starve every other background job. Each gets a thread of its own.
        let (done, result) = bounded(1);
        let spawned = std::thread::Builder::new()
            .name("workflow-command".into())
            .spawn(move || {
                let _ = done.send_blocking(command::run_command(&id, &command, &cwd, timeout_ms));
            });
        Box::pin(async move {
            if let Err(error) = spawned {
                return Err(format!("Could not start the command: {error}"));
            }
            result
                .recv()
                .await
                .unwrap_or_else(|_| Err("The command ended without a result".into()))
        })
    }

    fn cancel_command(&self, id: &str) -> Fut<Result<bool, String>> {
        let cancelled = command::cancel(id);
        Box::pin(async move { Ok(cancelled) })
    }

    fn ask_mono(&self, mono_id: &str, prompt: String) -> Fut<Result<String, String>> {
        let (monos, mut cx, mono_id) = (self.monos.clone(), self.cx.clone(), mono_id.to_owned());
        Box::pin(async move {
            let (tx, rx) = bounded(1);
            once_chat_is_ready(&monos, &mut cx, |m, cx| {
                m.workflow_ask(&mono_id, prompt.clone(), tx.clone(), cx)
            })
            .await?;
            rx.recv().await.unwrap_or_else(|_| closing())
        })
    }

    fn post(&self, report: Posted) {
        self.with(|m, cx| m.workflow_post(report, cx));
    }

    fn project_path(&self, mono_id: &str, input: &str) -> Option<String> {
        let named = input.trim();
        if named.is_empty() {
            return None;
        }
        self.monos
            .read_with(&self.cx, |m, _| {
                m.roster.get(mono_id)?.project(Some(named)).ok()
            })
            .ok()
            .flatten()
    }

    fn worktree_of(&self, session_id: &str) -> Option<String> {
        self.with(|m, cx| {
            let (_, view) = m.workflow_view(session_id, cx)?;
            let view = view.read(cx);
            Some(
                view.settings
                    .worktree_cwd
                    .clone()
                    .unwrap_or_else(|| view.summary.cwd.clone()),
            )
        })
        .flatten()
    }

    fn save_run(&self, mono_id: &str, run: WorkflowRun) -> Fut<Result<(), String>> {
        let root = self.monos.read_with(&self.cx, |m, _| m.root.clone());
        let executor = self.cx.background_executor().clone();
        let mono_id = mono_id.to_owned();
        Box::pin(async move {
            let Ok(root) = root else { return closing() };
            executor
                .spawn(async move {
                    let dir = MonoDir::new(root, mono_id);
                    update_runs(&dir, &run.workflow_id.clone(), |runs| {
                        let list = if runs.iter().any(|entry| entry.id == run.id) {
                            runs.into_iter()
                                .map(|entry| {
                                    if entry.id == run.id {
                                        run.clone()
                                    } else {
                                        entry
                                    }
                                })
                                .collect()
                        } else {
                            std::iter::once(run.clone()).chain(runs).collect()
                        };
                        Ok((list, ()))
                    })
                })
                .await
        })
    }
}
