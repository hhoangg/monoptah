//! Drives Mono workflow runs: the TS `executeRun` (`workflowRunner.ts`) on the pure engine.
//!
//! A run is one async task on the foreground executor. It asks the engine what is ready
//! (`next_actions`), executes it through a `WorkflowHost` (sessions, commands, the Mono, the
//! chat), feeds the results back (`apply_event`) and writes the newest state after every
//! event, until the engine says the run is over. It never panics: whatever goes wrong becomes
//! a failed node or a failed run.
//!
//! The host is a seam so the logic runs against a fake in tests; `host.rs` has the real one.
//! Scheduling (which workflow runs when, requests in the workflow files, the startup sweep)
//! is not here: it calls `WorkflowRunner::start`, `resume` and `stop`.
use gpui::AsyncApp;
use monocode_core::mono::workflows::{
    cli::Signals,
    engine::{
        EngineAction, EngineEvent, apply_event, is_asking, next_actions, resume_run, run_scope,
    },
    model::{
        Checkout, Cwd, DEFAULT_COMMAND_TIMEOUT_MINUTES, DEFAULT_SESSION_TIMEOUT_MINUTES, Node,
        NodeKind, NodeRun, NodeStatus, RunStatus, WorkflowRun,
    },
    ordered::{Json, OrderedMap, js_number_text},
    output::{output_reask, parse_workflow_output},
    report::{ask_mono_prompt, compose_report, parse_mono_decision},
    session_context::session_context,
    template::render_template,
};
use monocode_core::telegram::forward::Workflow as SessionContext;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap},
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use sync::{Either, Gate, Nudge, Signal, join_all, race};

mod api;
mod host;
mod sync;
#[cfg(test)]
mod tests;
mod text;

pub(crate) use api::*;
pub(super) use host::Live;
use text::{cap_map, cap_string, clip, run_folder_name, utf16_tail};

pub(crate) type Fut<T> = Pin<Box<dyn Future<Output = T>>>;

/// Where each session a live run uses came from (workflow, node, item, round), by persisted
/// session id, for the Telegram forwarder. Set once the node's session id is known and cleared
/// when the run ends.
type Contexts = Rc<RefCell<HashMap<String, SessionContext>>>;

const STOPPED_BY_USER: &str = "Stopped by the user.";
/// A cancel can beat the host registering the command, so it is retried briefly.
const CANCEL_ATTEMPTS: usize = 10;
const CANCEL_RETRY_MS: u64 = 50;
/// How much of a failed command's output goes into the node error.
const COMMAND_DETAIL_MAX: usize = 500;

fn now() -> u64 {
    monocode_core::time::now_ms()
}

// --- one run ------------------------------------------------------------------------------------

struct State {
    run: WorkflowRun,
    command_seq: u32,
    /// Ids of the commands this run has started and not yet heard back from.
    commands: BTreeSet<String>,
    /// The run's folder in each project it has started a session in.
    folders: HashMap<String, String>,
    dirty: bool,
    writing: bool,
}

struct Exec {
    host: Rc<dyn WorkflowHost>,
    cx: AsyncApp,
    mono_id: String,
    workflow_id: String,
    limits: Limits,
    /// Tells this execution's command ids from those of an earlier one of the same run.
    nonce: String,
    state: RefCell<State>,
    /// Node tasks still going.
    active: Cell<usize>,
    /// Settles when the user stops the run, so waits on a session end at once.
    stop: Signal,
    stopping: Cell<bool>,
    stopped: Signal,
    nudge: Nudge,
    flushed: Nudge,
    /// Sessions start one at a time so the first makes the run's folder and the rest reuse it.
    starts: Gate,
    registry: Registry,
    signals: Arc<Signals>,
    contexts: Contexts,
    /// The app is quitting: the run writes nothing more, so the file keeps saying
    /// `running` and the next start interrupts it (resumable) instead of failing or stopping it.
    quitting: Cell<bool>,
}

type Registry = Rc<RefCell<HashMap<String, Rc<Exec>>>>;

impl Exec {
    fn live(&self) -> bool {
        !self.quitting.get() && self.state.borrow().run.status == RunStatus::Running
    }

    /// The app is going down. Waiters end at once and the run is no longer `live`, so whatever the
    /// sessions and commands report from now on changes nothing; `save` writes nothing more.
    fn quit(&self) {
        if self.quitting.replace(true) {
            return;
        }
        self.stop.fire();
        self.nudge.nudge();
    }

    fn sleep(&self, ms: u64) -> gpui::Task<()> {
        self.cx
            .background_executor()
            .timer(Duration::from_millis(ms))
    }

    // --- persistence: one write at a time, always the newest state -----------------------------

    fn save(self: &Rc<Self>) {
        if self.quitting.get() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.dirty = true;
            if state.writing {
                return;
            }
            state.writing = true;
        }
        let this = self.clone();
        self.cx.spawn(async move |_| this.flush().await).detach();
    }

    async fn flush(self: Rc<Self>) {
        loop {
            let latest = {
                let mut state = self.state.borrow_mut();
                if !state.dirty {
                    state.writing = false;
                    break;
                }
                state.dirty = false;
                state.run.clone()
            };
            let id = latest.id.clone();
            if let Err(error) = self.host.save_run(&self.mono_id, latest).await {
                eprintln!("Could not save workflow run {id}: {error}");
                // The run goes on in memory; the record says its history is incomplete.
                let mut state = self.state.borrow_mut();
                if !state
                    .run
                    .error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("Could not save"))
                {
                    let note = format!("Could not save the run history: {error}");
                    state.run.error =
                        Some(match state.run.error.take().filter(|e| !e.is_empty()) {
                            Some(old) => format!("{note} {old}"),
                            None => note,
                        });
                }
            }
        }
        self.flushed.nudge();
    }

    async fn saved(&self) {
        while self.state.borrow().writing {
            self.flushed.wait().await;
        }
    }

    // --- events ---------------------------------------------------------------------------------

    fn apply(self: &Rc<Self>, event: EngineEvent) {
        {
            let mut state = self.state.borrow_mut();
            let next = apply_event(&state.run, &event);
            state.run = next;
        }
        self.save();
    }

    fn patch_node(self: &Rc<Self>, node_id: &str, branch: &str, patch: impl FnOnce(&mut NodeRun)) {
        {
            let mut state = self.state.borrow_mut();
            if let Some(node) = state
                .run
                .nodes
                .iter_mut()
                .find(|n| n.node_id == node_id && n.branch == branch)
            {
                patch(node);
            }
        }
        self.save();
    }

    fn fail(self: &Rc<Self>, node_id: &str, branch: &str, error: impl AsRef<str>) {
        self.apply(EngineEvent::NodeFailed {
            node_id: node_id.into(),
            branch: branch.into(),
            error: clip(error.as_ref()),
            at: now(),
        });
    }

    // --- lookups --------------------------------------------------------------------------------

    fn node_of(&self, node_id: &str) -> Option<Node> {
        self.state
            .borrow()
            .run
            .definition
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .cloned()
    }

    /// The session a node ran in this branch, or at the top level.
    fn session_of(&self, node_id: &str, branch: &str) -> Option<String> {
        let state = self.state.borrow();
        let find = |wanted: &str| {
            state
                .run
                .nodes
                .iter()
                .find(|n| n.node_id == node_id && n.branch == wanted && n.session_id.is_some())
                .and_then(|n| n.session_id.clone())
        };
        find(branch).or_else(|| find(""))
    }

    fn project_for(&self, template: &str, node_id: &str, branch: &str) -> Option<String> {
        let rendered = {
            let state = self.state.borrow();
            render_template(
                template,
                &run_scope(&state.run, branch, Some(node_id)),
                false,
            )
        };
        self.host.project_path(&self.mono_id, &rendered)
    }

    // --- waiting on a turn ------------------------------------------------------------------------

    /// Waits for a turn, showing any approval it asks for. Only time spent working counts toward
    /// the timeout; time waiting on the user has its own, longer limit.
    async fn watch_turn(
        self: &Rc<Self>,
        node_id: &str,
        branch: &str,
        session_id: &str,
        mut turn: Fut<Outcome>,
        timeout_ms: u64,
    ) -> Outcome {
        let poll = self.limits.poll_ms;
        let (mut working, mut waiting, mut asking) = (0u64, 0u64, false);
        let result = loop {
            let tick = self.sleep(poll);
            if let Either::Left(Either::Left(outcome)) =
                race(race(&mut turn, self.stop.wait()), tick).await
            {
                break outcome;
            }
            if !self.live() {
                break Outcome::cancelled(STOPPED_BY_USER);
            }
            let pending = self.host.waiting_approval(session_id);
            if pending {
                let (workflow_id, workflow_name, run_id) = {
                    let state = self.state.borrow();
                    (
                        state.run.workflow_id.clone(),
                        state.run.definition.name.clone(),
                        state.run.id.clone(),
                    )
                };
                self.host.ask_approval(ApprovalAsk {
                    mono_id: self.mono_id.clone(),
                    workflow_id,
                    workflow_name,
                    run_id,
                    session_id: session_id.into(),
                });
            }
            if pending != asking {
                asking = pending;
                self.apply(EngineEvent::WaitingApproval {
                    node_id: node_id.into(),
                    branch: branch.into(),
                    waiting: asking,
                });
            }
            if pending {
                waiting += poll;
            } else {
                working += poll;
            }
            let approval_expired = waiting >= self.limits.approval_wait_ms;
            if working >= timeout_ms || approval_expired {
                let error = if approval_expired {
                    "Nobody answered its approval request.".to_string()
                } else {
                    let minutes = (timeout_ms as f64 / 60_000.0 + 0.5).floor();
                    format!(
                        "It took longer than {} minutes and was stopped.",
                        js_number_text(minutes)
                    )
                };
                let _ = self.host.stop(session_id).await;
                break Outcome::failed(error);
            }
        };
        if asking && self.live() {
            self.apply(EngineEvent::WaitingApproval {
                node_id: node_id.into(),
                branch: branch.into(),
                waiting: false,
            });
        }
        self.host.end_approvals(session_id);
        result
    }

    // --- node execution -----------------------------------------------------------------------------

    async fn run_session(
        self: &Rc<Self>,
        node_id: String,
        branch: String,
        prompt: String,
        reuse_session_id: Option<String>,
    ) {
        let Some(Node {
            kind:
                NodeKind::Session {
                    project,
                    checkout,
                    output,
                    harness,
                    model,
                    effort,
                    nest_under,
                    timeout_minutes,
                    ..
                },
            ..
        }) = self.node_of(&node_id)
        else {
            return;
        };
        let Some(project_path) = self.project_for(&project, &node_id, &branch) else {
            return self.fail(
                &node_id,
                &branch,
                format!("\"{project}\" is not one of the Mono's projects"),
            );
        };
        let timeout_ms =
            (timeout_minutes.unwrap_or(DEFAULT_SESSION_TIMEOUT_MINUTES as f64) * 60_000.0) as u64;

        let (session_id, turn): (String, Fut<Outcome>) = if let Some(reuse) = reuse_session_id {
            let turn = self.host.send(&reuse, prompt);
            (reuse, turn)
        } else {
            let mut worktree_cwd = None;
            if let Checkout::WorktreeOf(source_node) = &checkout {
                worktree_cwd = self
                    .session_of(source_node, &branch)
                    .and_then(|source| self.host.worktree_of(&source));
                if worktree_cwd.is_none() {
                    return self.fail(
                        &node_id,
                        &branch,
                        format!("No worktree is available from \"{source_node}\""),
                    );
                }
            }
            let parent_session_id = nest_under
                .as_ref()
                .and_then(|parent| self.session_of(parent, &branch));
            let (folder_id, folder_name) = {
                let state = self.state.borrow();
                (
                    state
                        .folders
                        .get(&project_path)
                        .cloned()
                        .or_else(|| state.run.folder_id.clone()),
                    run_folder_name(&state.run.definition.name, state.run.started_at),
                )
            };
            let request = SessionRequest {
                project: project_path.clone(),
                prompt,
                harness: harness.filter(|h| !h.is_empty()),
                model: model.filter(|m| !m.is_empty()),
                effort: effort.filter(|e| !e.is_empty()),
                new_worktree: checkout == Checkout::NewWorktree,
                worktree_cwd,
                parent_session_id,
                folder_id,
                folder_name: Some(folder_name),
            };
            // The start runs on its own so Stop does not wait for one that hangs.
            let (done, started) = async_channel::bounded(1);
            let this = self.clone();
            self.cx
                .spawn(async move |_| {
                    let _turn = this.starts.turn().await;
                    // Starts queued behind another one wait here; a stopped run must not launch them.
                    let result = if this.live() {
                        this.host
                            .start_session(&this.mono_id, request)
                            .await
                            .map(Some)
                    } else {
                        Ok(None)
                    };
                    let _ = done.send(result).await;
                })
                .detach();
            let started = match race(started.recv(), self.stop.wait()).await {
                Either::Right(()) => {
                    // One that comes through later is ended.
                    let this = self.clone();
                    self.cx
                        .spawn(async move |_| {
                            if let Ok(Ok(Some(late))) = started.recv().await {
                                let _ = this.host.stop(&late.session_id).await;
                                this.host.end_approvals(&late.session_id);
                                this.host.release_session(&late.session_id);
                            }
                        })
                        .detach();
                    return;
                }
                Either::Left(Err(_)) | Either::Left(Ok(Ok(None))) => return,
                Either::Left(Ok(Err(error))) => return self.fail(&node_id, &branch, error),
                Either::Left(Ok(Ok(Some(started)))) => started,
            };
            if let Some(folder) = started.folder_id {
                let mut state = self.state.borrow_mut();
                state.folders.insert(project_path.clone(), folder.clone());
                if state.run.folder_id.is_none() {
                    state.run.folder_id = Some(folder);
                    drop(state);
                    self.save();
                }
            }
            if !self.live() {
                // Stopped while it was starting: nothing else knows this session yet.
                let _ = self.host.stop(&started.session_id).await;
                self.host.end_approvals(&started.session_id);
                self.host.release_session(&started.session_id);
                return;
            }
            (started.session_id, started.settled)
        };
        let session = session_id.clone();
        self.patch_node(&node_id, &branch, |n| n.session_id = Some(session));
        let context = session_context(&self.state.borrow().run, &node_id, &branch);
        self.contexts
            .borrow_mut()
            .insert(session_id.clone(), context);

        let mut outcome = self
            .watch_turn(&node_id, &branch, &session_id, turn, timeout_ms)
            .await;
        if !self.live() {
            return;
        }
        if outcome.status != Status::Completed {
            return self.fail(
                &node_id,
                &branch,
                outcome.error.unwrap_or_else(|| {
                    if outcome.status == Status::Cancelled {
                        "The session was stopped".into()
                    } else {
                        "The session did not finish".into()
                    }
                }),
            );
        }
        let reply_of = |done: &Outcome| {
            let reply = self.host.final_reply(&session_id);
            if reply.is_empty() {
                done.text.clone()
            } else {
                reply
            }
        };

        // A node with nothing declared has no contract to enforce, whatever its reply ends with.
        if output.is_empty() {
            return self.apply(EngineEvent::NodeFinished {
                node_id,
                branch,
                output: OrderedMap::new(),
                at: now(),
            });
        }
        let mut parsed = parse_workflow_output(&reply_of(&outcome), &output);
        if let Err(problem) = &parsed {
            self.patch_node(&node_id, &branch, |n| n.reasked = Some(true));
            let turn = self.host.send(&session_id, output_reask(&output, problem));
            outcome = self
                .watch_turn(&node_id, &branch, &session_id, turn, timeout_ms)
                .await;
            if !self.live() {
                return;
            }
            if outcome.status != Status::Completed {
                return self.fail(
                    &node_id,
                    &branch,
                    outcome.error.unwrap_or_else(|| problem.clone()),
                );
            }
            parsed = parse_workflow_output(&reply_of(&outcome), &output);
        }
        match parsed {
            Err(problem) => self.fail(&node_id, &branch, problem),
            Ok(value) => self.apply(EngineEvent::NodeFinished {
                node_id,
                branch,
                output: cap_map(&value),
                at: now(),
            }),
        }
    }

    async fn run_command(self: &Rc<Self>, node_id: String, branch: String, command: String) {
        let Some(Node {
            kind:
                NodeKind::Command {
                    project,
                    cwd,
                    timeout_minutes,
                    ..
                },
            ..
        }) = self.node_of(&node_id)
        else {
            return;
        };
        let Some(mut dir) = self.project_for(&project, &node_id, &branch) else {
            return self.fail(
                &node_id,
                &branch,
                format!("\"{project}\" is not one of the Mono's projects"),
            );
        };
        if let Some(Cwd::WorktreeOf(source_node)) = &cwd {
            match self
                .session_of(source_node, &branch)
                .and_then(|source| self.host.worktree_of(&source))
            {
                Some(worktree) => dir = worktree,
                None => {
                    return self.fail(
                        &node_id,
                        &branch,
                        format!("No worktree is available from \"{source_node}\""),
                    );
                }
            }
        }
        let minutes = timeout_minutes.unwrap_or(DEFAULT_COMMAND_TIMEOUT_MINUTES as f64);
        let id = {
            let mut state = self.state.borrow_mut();
            state.command_seq += 1;
            let id = format!(
                "{}:{}:{node_id}:{branch}:{}",
                state.run.id, self.nonce, state.command_seq
            );
            state.commands.insert(id.clone());
            id
        };
        let result = self
            .host
            .run_command(dir, command, (minutes * 60_000.0) as u64, id.clone())
            .await;
        self.state.borrow_mut().commands.remove(&id);
        if !self.live() {
            return;
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => return self.fail(&node_id, &branch, error),
        };
        if result.cancelled {
            return self.fail(&node_id, &branch, "The command was cancelled");
        }
        if result.timed_out {
            return self.fail(
                &node_id,
                &branch,
                format!(
                    "The command timed out after {} minutes",
                    js_number_text(minutes)
                ),
            );
        }
        if result.exit_code != Some(0) {
            let detail = [result.stderr.trim(), result.stdout.trim()]
                .into_iter()
                .find(|text| !text.is_empty())
                .unwrap_or("");
            let code = result
                .exit_code
                .map_or("unknown".to_string(), |code| code.to_string());
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", utf16_tail(detail, COMMAND_DETAIL_MAX))
            };
            return self.fail(
                &node_id,
                &branch,
                format!("The command exited with code {code}{detail}"),
            );
        }
        let mut output = OrderedMap::new();
        output.insert("exitCode", Json::Number(0.0));
        output.insert("stdout", Json::String(result.stdout));
        output.insert("stderr", Json::String(result.stderr));
        self.apply(EngineEvent::NodeFinished {
            node_id,
            branch,
            output: cap_map(&output),
            at: now(),
        });
    }

    async fn run_ask(self: &Rc<Self>, action: &EngineAction) {
        let EngineAction::AskMono {
            node_id,
            branch,
            answers,
            ..
        } = action
        else {
            return;
        };
        let name = self.state.borrow().run.definition.name.clone();
        let prompt = ask_mono_prompt(action, &name).unwrap_or_default();
        let asking = self.host.ask_mono(&self.mono_id, prompt);
        let deadline = self.sleep(self.limits.ask_timeout_ms);
        // Stop abandons the question (its hidden session ends on its own).
        let reply = match race(race(asking, deadline), self.stop.wait()).await {
            Either::Right(()) => return,
            Either::Left(Either::Right(())) => Err("The Mono did not answer in time".to_string()),
            Either::Left(Either::Left(reply)) => reply,
        };
        if !self.live() {
            return;
        }
        match reply.and_then(|reply| parse_mono_decision(&reply, answers)) {
            Err(error) => self.fail(node_id, branch, error),
            Ok(decision) => self.apply(EngineEvent::Decided {
                node_id: node_id.clone(),
                branch: branch.clone(),
                value: decision.value,
                reason: decision.reason.map(|reason| cap_string(&reason)),
                at: now(),
            }),
        }
    }

    /// Kills every command this run still has going. Never fails.
    async fn cancel_commands(self: &Rc<Self>) {
        let ids: Vec<String> = self.state.borrow().commands.iter().cloned().collect();
        join_all(
            ids.into_iter()
                .map(|id| {
                    let this = self.clone();
                    Box::pin(async move {
                        for _ in 0..CANCEL_ATTEMPTS {
                            if !this.state.borrow().commands.contains(&id) {
                                return;
                            }
                            match this.host.cancel_command(&id).await {
                                Ok(true) => return,
                                Ok(false) => {}
                                Err(error) => {
                                    eprintln!("Could not cancel command {id}: {error}");
                                    return;
                                }
                            }
                            this.sleep(CANCEL_RETRY_MS).await;
                        }
                    }) as Fut<()>
                })
                .collect(),
        )
        .await;
    }

    fn track(self: &Rc<Self>, action: EngineAction) {
        self.active.set(self.active.get() + 1);
        let this = self.clone();
        self.cx
            .spawn(async move |_| {
                match action {
                    EngineAction::StartSession {
                        node_id,
                        branch,
                        prompt,
                        reuse_session_id,
                        ..
                    } => {
                        this.run_session(node_id, branch, prompt, reuse_session_id)
                            .await
                    }
                    EngineAction::RunCommand {
                        node_id,
                        branch,
                        command,
                    } => this.run_command(node_id, branch, command).await,
                    ref ask @ EngineAction::AskMono { .. } => this.run_ask(ask).await,
                    EngineAction::PostReport => {}
                }
                this.active.set(this.active.get() - 1);
                this.nudge.nudge();
            })
            .detach();
    }

    // --- report -----------------------------------------------------------------------------------

    fn launches_of(&self, run: &WorkflowRun) -> Vec<SessionInfo> {
        let mut seen = BTreeSet::new();
        let mut launches = Vec::new();
        for node in &run.nodes {
            let Some(id) = &node.session_id else { continue };
            if !seen.insert(id.clone()) {
                continue;
            }
            launches.extend(self.host.session_info(id));
        }
        launches
    }

    fn post_text(&self, finished: &WorkflowRun) {
        let report = compose_report(finished);
        self.host.post(Posted {
            mono_id: self.mono_id.clone(),
            workflow_id: finished.workflow_id.clone(),
            workflow_name: finished.definition.name.clone(),
            run_id: finished.id.clone(),
            title: report.title,
            text: report.text,
            launches: self.launches_of(finished),
        });
    }

    /// Posts the report. The run's final status is known once the report counts as posted.
    fn post_report(self: &Rc<Self>) -> bool {
        let posted = {
            let mut state = self.state.borrow_mut();
            let posted = apply_event(&state.run, &EngineEvent::ReportPosted { at: now() });
            if posted == state.run {
                return false;
            }
            state.run = posted.clone();
            posted
        };
        self.save();
        self.post_text(&posted);
        true
    }

    /// Starts everything the engine has ready. A question or session is marked started first.
    fn pump(self: &Rc<Self>) {
        for _ in 0..1_000 {
            if !self.live() {
                return;
            }
            let action = {
                let state = self.state.borrow();
                next_actions(&state.run).into_iter().next()
            };
            let Some(action) = action else { return };
            let (node_id, branch, session_id) = match &action {
                EngineAction::PostReport => {
                    if !self.post_report() {
                        return;
                    }
                    continue;
                }
                EngineAction::StartSession {
                    node_id,
                    branch,
                    reuse_session_id,
                    ..
                } => (node_id, branch, reuse_session_id.clone()),
                EngineAction::RunCommand {
                    node_id, branch, ..
                }
                | EngineAction::AskMono {
                    node_id, branch, ..
                } => (node_id, branch, None),
            };
            let before = self.state.borrow().run.clone();
            self.apply(EngineEvent::NodeStarted {
                node_id: node_id.clone(),
                branch: branch.clone(),
                session_id,
                at: now(),
            });
            // The engine refused: do not loop on an action it will keep offering.
            if self.state.borrow().run == before {
                return;
            }
            self.track(action);
        }
    }

    // --- stop -------------------------------------------------------------------------------------

    /// Stops the run: ends it first (so whatever the sessions report as they stop changes
    /// nothing), then its commands and sessions, and posts the report once.
    fn request_stop(self: &Rc<Self>) -> bool {
        if !self.live() || self.stopping.replace(true) {
            return false;
        }
        let this = self.clone();
        self.cx
            .spawn(async move |_| {
                this.stop_run().await;
                this.stopped.fire();
            })
            .detach();
        true
    }

    async fn stop_run(self: &Rc<Self>) {
        let mut sessions: Vec<String> = Vec::new();
        {
            let state = self.state.borrow();
            for node in &state.run.nodes {
                let busy = matches!(
                    node.status,
                    NodeStatus::Running | NodeStatus::WaitingApproval
                );
                if let Some(id) = &node.session_id
                    && busy
                    && !is_asking(node)
                    && !sessions.contains(id)
                {
                    sessions.push(id.clone());
                }
            }
        }
        self.apply(EngineEvent::Stopped { at: now() });
        self.stop.fire();
        self.nudge.nudge();
        let mut work: Vec<Fut<()>> = vec![{
            let this = self.clone();
            Box::pin(async move { this.cancel_commands().await })
        }];
        for id in sessions {
            let this = self.clone();
            work.push(Box::pin(async move {
                if let Err(error) = this.host.stop(&id).await {
                    eprintln!("Could not stop session {id}: {error}");
                }
                this.host.end_approvals(&id);
            }));
        }
        join_all(work).await;
        // A stopped run reports once, with what had finished by then.
        let report = {
            let mut state = self.state.borrow_mut();
            if state.run.report_posted_at.is_none() {
                state.run.report_posted_at = Some(now());
                Some(state.run.clone())
            } else {
                None
            }
        };
        if let Some(run) = report {
            self.save();
            self.post_text(&run);
        }
    }

    // --- the run ----------------------------------------------------------------------------------

    async fn execute(self: Rc<Self>) -> WorkflowRun {
        // The record exists before anything runs, so a failure to save is known up front.
        self.save();
        self.saved().await;
        while self.live() {
            self.pump();
            if !self.live() {
                break;
            }
            if self.active.get() == 0 {
                {
                    let mut state = self.state.borrow_mut();
                    let note = "The run could not make progress.";
                    state.run.status = RunStatus::Failed;
                    state.run.ended_at = Some(now());
                    state.run.error =
                        Some(match state.run.error.take().filter(|e| !e.is_empty()) {
                            Some(old) => format!("{old} {note}"),
                            None => note.into(),
                        });
                }
                self.save();
                break;
            }
            self.nudge.wait().await;
        }
        // Stop ends commands and sessions at once, a session notices within a poll and a command
        // has its node timeout. A question to the Mono and a start that has not returned are
        // abandoned on Stop, so they do not hold the run; a run that ended some other way waits
        // for them. Then the workflow is not seen as idle (and a scheduled run does not start in
        // the same directory) while something of this run is still alive.
        if self.stopping.get() {
            self.stopped.wait().await;
        }
        self.cancel_commands().await;
        while self.active.get() > 0 {
            self.nudge.wait().await;
        }
        self.saved().await;
        let run = self.state.borrow().run.clone();
        let mut sessions: Vec<&String> = Vec::new();
        for node in &run.nodes {
            let past = node
                .history
                .iter()
                .flatten()
                .filter_map(|r| r.session_id.as_ref());
            for id in node.session_id.iter().chain(past) {
                if !sessions.contains(&id) {
                    sessions.push(id);
                }
            }
        }
        for id in sessions {
            self.host.end_approvals(id);
            self.host.release_session(id);
            self.contexts.borrow_mut().remove(id);
        }
        self.signals.set_running(&self.workflow_id, false);
        self.registry.borrow_mut().remove(&self.workflow_id);
        run
    }
}

// --- the runner ---------------------------------------------------------------------------------

/// The live runs of the app, one per workflow. Clones share the runs: the quit path keeps one so
/// it can reach the runner without updating `Monos`.
#[derive(Clone)]
pub(crate) struct WorkflowRunner {
    live: Registry,
    signals: Arc<Signals>,
    limits: Limits,
    contexts: Contexts,
    /// The app is quitting: nothing new starts.
    quitting: Rc<Cell<bool>>,
}

impl WorkflowRunner {
    pub(crate) fn new(signals: Arc<Signals>) -> Self {
        Self {
            live: Default::default(),
            signals,
            limits: Limits::default(),
            contexts: Default::default(),
            quitting: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub(crate) fn is_running(&self, workflow_id: &str) -> bool {
        self.live.borrow().contains_key(workflow_id)
    }

    /// Reads the workflow's run while the runner has it, without copying it out.
    pub(crate) fn with_run<R>(
        &self,
        workflow_id: &str,
        read: impl FnOnce(&WorkflowRun) -> R,
    ) -> Option<R> {
        let exec = self.live.borrow().get(workflow_id).cloned()?;
        let state = exec.state.borrow();
        Some(read(&state.run))
    }

    /// The runs going, as `(workflow id, run id)`.
    pub(crate) fn running(&self) -> Vec<(String, String)> {
        self.live
            .borrow()
            .iter()
            .map(|(workflow, exec)| (workflow.clone(), exec.state.borrow().run.id.clone()))
            .collect()
    }

    /// Runs `run` (made by the engine's `start_run` or `resume_run`) for the Mono. False when the
    /// workflow already has a run going; a run that is already over is only written.
    pub(crate) fn start(
        &self,
        host: Rc<dyn WorkflowHost>,
        cx: &AsyncApp,
        mono_id: &str,
        run: WorkflowRun,
    ) -> bool {
        let workflow_id = run.workflow_id.clone();
        if self.quitting.get() || self.is_running(&workflow_id) {
            return false;
        }
        let exec = Rc::new(Exec {
            host,
            cx: cx.clone(),
            mono_id: mono_id.into(),
            workflow_id: workflow_id.clone(),
            limits: self.limits,
            nonce: monocode_core::mono::new_id().chars().take(6).collect(),
            state: RefCell::new(State {
                run,
                command_seq: 0,
                commands: BTreeSet::new(),
                folders: HashMap::new(),
                dirty: false,
                writing: false,
            }),
            active: Cell::new(0),
            stop: Signal::default(),
            stopping: Cell::new(false),
            stopped: Signal::default(),
            nudge: Nudge::default(),
            flushed: Nudge::default(),
            starts: Gate::default(),
            registry: self.live.clone(),
            signals: self.signals.clone(),
            contexts: self.contexts.clone(),
            quitting: Cell::new(false),
        });
        self.live
            .borrow_mut()
            .insert(workflow_id.clone(), exec.clone());
        self.signals.set_running(&workflow_id, true);
        cx.spawn(async move |_| {
            exec.execute().await;
        })
        .detach();
        true
    }

    /// Continues an interrupted run from where it stopped. False when the workflow is running or
    /// the run was not interrupted.
    pub(crate) fn resume(
        &self,
        host: Rc<dyn WorkflowHost>,
        cx: &AsyncApp,
        mono_id: &str,
        stored: &WorkflowRun,
    ) -> bool {
        stored.status == RunStatus::Interrupted && self.start(host, cx, mono_id, resume_run(stored))
    }

    /// Stops the workflow's run. False when none is going or it is already stopping.
    pub(crate) fn stop(&self, workflow_id: &str) -> bool {
        let exec = self.live.borrow().get(workflow_id).cloned();
        exec.is_some_and(|exec| exec.request_stop())
    }

    /// Stops the runs of a Mono (it was deleted) like the user's Stop does: they end as stopped
    /// and their sessions and commands are stopped. Returns how many were asked to stop.
    pub(crate) fn stop_mono(&self, mono_id: &str) -> usize {
        let of_mono: Vec<_> = self
            .live
            .borrow()
            .values()
            .filter(|exec| exec.mono_id == mono_id)
            .cloned()
            .collect();
        of_mono.iter().filter(|exec| exec.request_stop()).count()
    }

    /// The app is quitting. Every run lets go at once and writes nothing more: its file keeps
    /// saying `running`, so the next start marks it interrupted and it can be resumed. Nothing new
    /// starts afterwards. Nothing is posted (a quit is no stop) and no node fails because a command or session was killed with
    /// the app. Commands are killed by the caller (`command::cancel_all`), not waited for.
    pub(crate) fn quit(&self) {
        self.quitting.set(true);
        let all: Vec<_> = self.live.borrow().values().cloned().collect();
        for exec in all {
            exec.quit();
        }
    }

    pub(crate) fn is_quitting(&self) -> bool {
        self.quitting.get()
    }

    /// Where the session came from, while a live run uses it.
    pub(crate) fn context(&self, persisted_session_id: &str) -> Option<SessionContext> {
        self.contexts.borrow().get(persisted_session_id).cloned()
    }
}

// --- what the scheduler calls -------------------------------------------------------------------

impl super::Monos {
    /// The Mono is deleted: its workflow runs stop (as Stop does) and so do its questions to it.
    pub(super) fn stop_workflows_of(&mut self, mono_id: &str, cx: &mut gpui::Context<Self>) {
        self.workflows.stop_mono(mono_id);
        let asks: Vec<i64> = self
            .workflow_live
            .asks
            .keys()
            .copied()
            .filter(|&session| self.workflow_live.asked_by(session) == Some(mono_id))
            .collect();
        for session in asks {
            self.workflow_ask_end(session, Err("The Mono was deleted".into()), cx);
        }
    }

    /// A live run's session context, as the run sets it (the Telegram tests have no run).
    #[cfg(test)]
    pub(crate) fn remember_workflow_context(&mut self, persisted: String, context: SessionContext) {
        self.workflows
            .contexts
            .borrow_mut()
            .insert(persisted, context);
    }

    /// Where a session a live workflow run uses came from, for Telegram.
    pub(crate) fn workflow_context(&self, persisted_session_id: &str) -> Option<SessionContext> {
        self.workflows.context(persisted_session_id)
    }

    /// Runs `run` for the Mono. False when the workflow already has a run going.
    pub(super) fn start_workflow_run(
        &mut self,
        mono_id: &str,
        run: WorkflowRun,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let host = self.workflow_host(cx);
        self.workflows.start(host, &cx.to_async(), mono_id, run)
    }

    /// Continues an interrupted run. False when the workflow is running or the run was not
    /// interrupted.
    pub(super) fn resume_workflow_run(
        &mut self,
        mono_id: &str,
        stored: &WorkflowRun,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let host = self.workflow_host(cx);
        self.workflows.resume(host, &cx.to_async(), mono_id, stored)
    }

    /// Stops the workflow's run. False when none is going.
    pub(super) fn stop_workflow_run(&mut self, workflow_id: &str) -> bool {
        self.workflows.stop(workflow_id)
    }
}
