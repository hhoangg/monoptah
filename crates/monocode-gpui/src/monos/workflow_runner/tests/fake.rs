//! A fake host: sessions that answer from a script, commands that return what they are told, and a
//! log of everything the run asked for.
use super::*;
use std::cell::Ref;

pub(super) struct Sess {
    id: String,
    cwd: String,
    reply: String,
    waiting: bool,
}

#[derive(Default)]
pub(super) struct World {
    sessions: Vec<Sess>,
    replies: VecDeque<String>,
    answers: VecDeque<Option<Result<String, String>>>,
    commands_to_return: VecDeque<Result<CommandResult, String>>,
    hold: Option<Receiver<()>>,
    /// Senders of the gates, so a gate stays shut until a test closes it.
    kept: Vec<Sender<()>>,
    command_gate: Option<Receiver<()>>,
    start_gate: Option<(usize, Receiver<()>)>,
    start_error: Option<String>,
    turn_outcome: Option<Outcome>,
    no_worktrees: bool,
    stop_fails: bool,
    cancel_misses: usize,
    save_error: Option<String>,
    save_delay: Option<u64>,
    executor: Option<gpui::BackgroundExecutor>,
    inflight: usize,
    start_calls: usize,
    pub(super) max_inflight: usize,
    pub(super) starts: Vec<SessionRequest>,
    pub(super) sends: Vec<(String, String)>,
    pub(super) stops: Vec<String>,
    pub(super) ended: Vec<String>,
    pub(super) released: Vec<String>,
    pub(super) approvals: Vec<ApprovalAsk>,
    pub(super) asks: Vec<(String, String)>,
    pub(super) commands: Vec<(String, String, u64, String)>,
    pub(super) cancelled: Vec<String>,
    pub(super) posts: Vec<Posted>,
    pub(super) saves: Vec<WorkflowRun>,
}

/// Releases whatever waits on it when closed (or dropped).
pub(super) struct Hold(Sender<()>);

impl Hold {
    pub(super) fn close(&self) {
        self.0.close();
    }
}

#[derive(Clone, Default)]
pub(super) struct FakeHost(Rc<RefCell<World>>);

impl FakeHost {
    pub(super) fn world(&self) -> Ref<'_, World> {
        self.0.borrow()
    }
    fn gate(&self) -> (Hold, Receiver<()>) {
        let (tx, rx) = unbounded();
        self.0.borrow_mut().kept.push(tx.clone());
        (Hold(tx), rx)
    }
    /// Turns wait for this to be closed before they answer.
    pub(super) fn hold(&self) -> Hold {
        let (hold, rx) = self.gate();
        self.0.borrow_mut().hold = Some(rx);
        hold
    }
    pub(super) fn command_gate(&self) -> Hold {
        let (hold, rx) = self.gate();
        self.0.borrow_mut().command_gate = Some(rx);
        hold
    }
    /// Every start waits for this.
    pub(super) fn start_gate(&self) -> Hold {
        self.start_gate_for(0)
    }
    /// The `n`th start (1 based; 0: all of them) waits for this.
    pub(super) fn start_gate_for(&self, n: usize) -> Hold {
        let (hold, rx) = self.gate();
        self.0.borrow_mut().start_gate = Some((n, rx));
        hold
    }
    pub(super) fn answer(&self, reply: Result<String, String>) {
        self.0.borrow_mut().answers.push_back(Some(reply));
    }
    pub(super) fn hang_next_ask(&self) {
        self.0.borrow_mut().answers.push_back(None);
    }
    pub(super) fn fail_starts(&self, error: &str) {
        self.0.borrow_mut().start_error = Some(error.into());
    }
    pub(super) fn end_turns_with(&self, outcome: Outcome) {
        self.0.borrow_mut().turn_outcome = Some(outcome);
    }
    pub(super) fn no_worktrees(&self) {
        self.0.borrow_mut().no_worktrees = true;
    }
    pub(super) fn stop_fails(&self) {
        self.0.borrow_mut().stop_fails = true;
    }
    pub(super) fn set_waiting(&self, session: &str, waiting: bool) {
        for s in &mut self.0.borrow_mut().sessions {
            if s.id == session {
                s.waiting = waiting;
            }
        }
    }
    pub(super) fn command_returns(&self, result: CommandResult) {
        self.0.borrow_mut().commands_to_return.push_back(Ok(result));
    }
    pub(super) fn command_fails(&self, error: &str) {
        self.0
            .borrow_mut()
            .commands_to_return
            .push_back(Err(error.into()));
    }
    /// The first `n` cancels find no such command (it has not registered yet).
    pub(super) fn cancel_refuses(&self, n: usize) {
        self.0.borrow_mut().cancel_misses = n;
    }
    pub(super) fn fail_saves(&self, error: &str) {
        self.0.borrow_mut().save_error = Some(error.into());
    }
    pub(super) fn slow_saves(&self, ms: u64) {
        self.0.borrow_mut().save_delay = Some(ms);
    }

    fn turn(&self, id: &str, _prompt: String) -> Fut<Outcome> {
        let host = self.clone();
        let id = id.to_string();
        if let Some(s) = host.0.borrow_mut().sessions.iter_mut().find(|s| s.id == id) {
            s.reply.clear();
        }
        Box::pin(async move {
            let hold = host.0.borrow().hold.clone();
            if let Some(hold) = hold {
                let _ = hold.recv().await;
            }
            let mut w = host.0.borrow_mut();
            let reply = w.replies.pop_front().unwrap_or_default();
            if let Some(s) = w.sessions.iter_mut().find(|s| s.id == id) {
                s.reply = reply.clone();
                s.waiting = false;
            }
            w.turn_outcome
                .clone()
                .unwrap_or_else(|| Outcome::completed(reply))
        })
    }
}

impl WorkflowHost for FakeHost {
    fn start_session(
        &self,
        _mono_id: &str,
        req: SessionRequest,
    ) -> Fut<Result<StartedSession, String>> {
        let host = self.clone();
        let call = {
            let mut w = host.0.borrow_mut();
            w.start_calls += 1;
            w.starts.push(req.clone());
            w.start_calls
        };
        Box::pin(async move {
            let gate = host
                .0
                .borrow()
                .start_gate
                .clone()
                .filter(|(n, _)| *n == 0 || *n == call)
                .map(|(_, rx)| rx);
            if let Some(gate) = gate {
                let _ = gate.recv().await;
            }
            if let Some(error) = host.0.borrow().start_error.clone() {
                return Err(error);
            }
            let id = {
                let mut w = host.0.borrow_mut();
                let id = format!("s{}", w.sessions.len() + 1);
                w.sessions.push(Sess {
                    id: id.clone(),
                    cwd: req.project.clone(),
                    reply: String::new(),
                    waiting: false,
                });
                id
            };
            let settled = host.turn(&id, req.prompt);
            Ok(StartedSession {
                session_id: id,
                folder_id: Some(format!("folder:{}", req.project)),
                settled,
            })
        })
    }

    fn send(&self, session_id: &str, prompt: String) -> Fut<Outcome> {
        self.0
            .borrow_mut()
            .sends
            .push((session_id.into(), prompt.clone()));
        self.turn(session_id, prompt)
    }

    fn stop(&self, session_id: &str) -> Fut<Result<(), String>> {
        let mut w = self.0.borrow_mut();
        w.stops.push(session_id.into());
        let result = if w.stop_fails {
            Err("It would not stop".into())
        } else {
            Ok(())
        };
        Box::pin(async move { result })
    }

    fn final_reply(&self, session_id: &str) -> String {
        let w = self.0.borrow();
        w.sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.reply.clone())
            .unwrap_or_default()
    }

    fn waiting_approval(&self, session_id: &str) -> bool {
        let w = self.0.borrow();
        w.sessions.iter().any(|s| s.id == session_id && s.waiting)
    }

    fn ask_approval(&self, ask: ApprovalAsk) {
        self.0.borrow_mut().approvals.push(ask);
    }

    fn end_approvals(&self, session_id: &str) {
        self.0.borrow_mut().ended.push(session_id.into());
    }

    fn release_session(&self, session_id: &str) {
        self.0.borrow_mut().released.push(session_id.into());
    }

    fn session_info(&self, session_id: &str) -> Option<SessionInfo> {
        let w = self.0.borrow();
        w.sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| SessionInfo {
                session_id: s.id.clone(),
                cwd: s.cwd.clone(),
                title: format!("Session {}", s.id),
                harness: "claude".into(),
                model: Some("claude:x".into()),
            })
    }

    fn run_command(
        &self,
        cwd: String,
        command: String,
        timeout_ms: u64,
        id: String,
    ) -> Fut<Result<CommandResult, String>> {
        let host = self.clone();
        host.0
            .borrow_mut()
            .commands
            .push((cwd, command, timeout_ms, id));
        Box::pin(async move {
            let gate = host.0.borrow().command_gate.clone();
            if let Some(gate) = gate {
                let _ = gate.recv().await;
            }
            host.0
                .borrow_mut()
                .commands_to_return
                .pop_front()
                .unwrap_or_else(|| Ok(command_result(Some(0), "fine", "")))
        })
    }

    fn cancel_command(&self, id: &str) -> Fut<Result<bool, String>> {
        let mut w = self.0.borrow_mut();
        w.cancelled.push(id.into());
        let hit = if w.cancel_misses > 0 {
            w.cancel_misses -= 1;
            false
        } else {
            true
        };
        Box::pin(async move { Ok(hit) })
    }

    fn ask_mono(&self, mono_id: &str, prompt: String) -> Fut<Result<String, String>> {
        let mut w = self.0.borrow_mut();
        w.asks.push((mono_id.into(), prompt));
        let answer = w
            .answers
            .pop_front()
            .unwrap_or_else(|| Some(Ok(fenced(json!({"answer": "exit"})))));
        Box::pin(async move {
            match answer {
                Some(answer) => answer,
                None => std::future::pending().await,
            }
        })
    }

    fn post(&self, report: Posted) {
        self.0.borrow_mut().posts.push(report);
    }

    fn project_path(&self, _mono_id: &str, input: &str) -> Option<String> {
        ["/code/app", "/code/lib"]
            .contains(&input)
            .then(|| input.to_string())
    }

    fn worktree_of(&self, session_id: &str) -> Option<String> {
        (!self.0.borrow().no_worktrees).then(|| format!("/code/app-wt-{session_id}"))
    }

    fn save_run(&self, _mono_id: &str, run: WorkflowRun) -> Fut<Result<(), String>> {
        let host = self.clone();
        let (delay, executor) = {
            let mut w = host.0.borrow_mut();
            w.inflight += 1;
            w.max_inflight = w.max_inflight.max(w.inflight);
            (w.save_delay, w.executor.clone())
        };
        Box::pin(async move {
            if let (Some(ms), Some(executor)) = (delay, executor) {
                executor.timer(Duration::from_millis(ms)).await;
            }
            let mut w = host.0.borrow_mut();
            w.inflight -= 1;
            if let Some(error) = w.save_error.clone() {
                return Err(error);
            }
            w.saves.push(run);
            Ok(())
        })
    }
}

/// A fake host, a runner over it and the signals they share.
pub(super) struct Rig {
    pub(super) fake: FakeHost,
    pub(super) runner: WorkflowRunner,
    pub(super) signals: Arc<Signals>,
}

impl Rig {
    pub(super) fn new(replies: &[&str]) -> Self {
        Self::with_limits(replies, Limits::default())
    }

    pub(super) fn with_limits(replies: &[&str], limits: Limits) -> Self {
        let fake = FakeHost::default();
        fake.0.borrow_mut().replies = replies.iter().map(|r| r.to_string()).collect();
        let signals = Arc::new(Signals::default());
        Self {
            fake,
            runner: WorkflowRunner::new(signals.clone()).with_limits(limits),
            signals,
        }
    }

    /// A manual run of `def` with the id `run-1`.
    pub(super) fn start(&self, cx: &TestAppContext, def: &Workflow) -> bool {
        self.start_run(cx, start_run(def, RunTrigger::Manual, 1_000, "run-1"))
    }

    pub(super) fn start_run(&self, cx: &TestAppContext, run: WorkflowRun) -> bool {
        self.fake.0.borrow_mut().executor = Some(cx.executor());
        self.runner
            .start(Rc::new(self.fake.clone()), &cx.to_async(), "mono-1", run)
    }

    /// The newest state the run was written in.
    pub(super) fn last_run(&self) -> WorkflowRun {
        self.fake
            .0
            .borrow()
            .saves
            .last()
            .cloned()
            .expect("the run was saved")
    }
}
