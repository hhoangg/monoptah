//! A session-bound terminal pane: the provider's own CLI shown in place of the
//! chat, for a session whose surface is the terminal. Ports `TuiSessionPane.tsx`
//! (launch, auto-claim of a never-used id, restart, exit bar, close on a clean
//! exit). The PTY attach/handoff a moved pane needs in the web app has no
//! counterpart: the entity lives in the shell's map, so a moved pane keeps its
//! process.
use crate::{
    runtime::{Database, locked},
    terminal::{Terminal, TerminalEvent},
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, Role, StyleRefinement,
    Subscription, Window, div, prelude::*, px,
};
use monocode_core::{
    accounts::{self as core_accounts, Book, Profile},
    provider::Kind,
    store::Session,
    tui_hook,
    tui_launch::{self, EARLY_EXIT, Exit, LaunchInput, TuiLaunch},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Instant,
};

#[cfg(test)]
mod tests;

/// A session that runs as its provider's CLI. A project on another machine has
/// no local terminal to run it in, so a remote row is never one.
pub(crate) fn runs_in_terminal(summary: &Session) -> bool {
    summary.surface.is_tui()
        && summary.id >= 0
        && !summary
            .cwd
            .replace('\\', "/")
            .starts_with(monocode_core::remote::REMOTE_PATH_PREFIX)
}

const NEW_WORKTREE_TIP: &str = "Not available in terminal sessions. A terminal session runs in the current checkout. Use a chat session to start in a new worktree.";

/// A plain text hover card.
struct Tip(&'static str);
impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let text = self.0;
        div()
            .debug_selector(move || format!("tui-tip:{text}"))
            .max_w(px(280.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(gpui::rgb(0x262626))
            .border_1()
            .border_color(content(0.12))
            .text_size(px(12.))
            .text_color(content(0.85))
            .child(self.0)
    }
}

const REMOVED_ACCOUNT: &str =
    "This session uses a removed provider account. Open a new session with another account.";

/// What the shell hears from a terminal session.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TuiEvent {
    /// The CLI process is running.
    Started,
    /// The pane settled on an exit (or on a launch that failed).
    Exited(Exit),
    /// The CLI's title, cleaned: words only, capped.
    Title(String),
    /// The turn state changed (hooks, or the CLI exiting).
    Busy(bool),
    /// The session's saved conversation id changed.
    Conversation(Option<String>),
    /// The CLI ran for a while and exited cleanly: the pane has nothing left to
    /// show. Always follows `Exited`, so an automation run settles first.
    Close,
}

/// Everything a launch needs from outside the UI thread's reach: the store, the
/// account book and the filesystem.
pub(crate) struct Request {
    pub store: Database,
    pub id: i64,
    pub kind: Kind,
    /// The project folder.
    pub cwd: String,
    /// The conversation the session saved.
    pub provider_id: Option<String>,
    /// Relaunch under `--session-id` with this saved id, which the CLI never wrote.
    pub claim: Option<String>,
    /// Ignore the saved conversation and start a new one.
    pub fresh: bool,
    /// The CLI's first prompt, for a conversation that has none yet.
    pub initial_prompt: Option<String>,
    pub hooks: bool,
    pub book: Book,
    pub root: PathBuf,
    pub binary: fn(Kind) -> Result<PathBuf, String>,
}

/// A launch ready to spawn; everything it decided is already saved.
pub(crate) struct Prepared {
    pub launch: TuiLaunch,
    /// The worktree unless it was removed, else the project folder.
    pub cwd: String,
    /// The session has a worktree (even a removed one): the header hides "New worktree".
    pub has_worktree: bool,
    pub env_set: Vec<(String, OsString)>,
    pub env_remove: Vec<&'static str>,
    /// The conversation id the session has now.
    pub provider_id: Option<String>,
    /// The launch uses an id saved earlier, by resuming or by claiming it.
    pub reused_id: bool,
    pub claimed: bool,
}

/// Decide and save a launch: pin the account, resolve the binary, build the
/// arguments and save a minted conversation id, all before anything spawns, so a
/// restart or a crash cannot lose the conversation. Blocks on the store and the
/// filesystem: call it off the UI thread.
pub(crate) fn prepare(request: Request) -> Result<Prepared, String> {
    let Request {
        store,
        id,
        kind,
        cwd,
        provider_id,
        claim,
        fresh,
        initial_prompt,
        hooks,
        book,
        root,
        binary,
    } = request;
    let saved = provider_id
        .filter(|saved| !saved.trim().is_empty() && !fresh)
        .map(|saved| saved.trim().to_owned());
    let (mut settings, data_dir) = {
        let store = locked(&store)?;
        (
            store.agent_settings(id)?.unwrap_or_default(),
            store.data_dir().map(Path::to_path_buf),
        )
    };
    // A conversation belongs to the account that started it, so the first
    // launch pins the selected account the way the first chat send does.
    let mut profile = Profile::default();
    let mut pin = false;
    if core_accounts::supports_accounts(kind) {
        profile = settings.account.clone();
        if profile.id == core_accounts::DEFAULT && saved.is_none() {
            let selected = book.selected(kind, &cwd);
            if selected != core_accounts::DEFAULT {
                profile = Profile::new(&selected, &root);
                pin = true;
            }
        }
        if profile.id != core_accounts::DEFAULT {
            if !book.exists(kind, &profile.id) {
                return Err(REMOVED_ACCOUNT.into());
            }
            profile = Profile::new(&profile.id, &root);
        }
    }
    let launch_cwd = match (&settings.worktree_cwd, settings.worktree_removed) {
        (Some(worktree), false) => worktree.clone(),
        _ => cwd,
    };
    let has_worktree = settings.worktree_cwd.is_some();
    let binary = binary(kind)?;
    // Busy and idle come from Claude Code hooks calling back into this
    // executable; without a data folder there is nowhere to spool them.
    let hook = (hooks && kind == Kind::Claude && !cfg!(windows))
        .then(|| {
            let cli = std::env::current_exe().ok()?;
            Some((cli, tui_hook::spool_dir(data_dir.as_deref()?)))
        })
        .flatten();
    let (env_set, env_remove) = profile.env(kind)?;
    let launch = tui_launch::build(LaunchInput {
        kind,
        binary,
        model: settings.model.clone(),
        access: settings.access,
        provider_id: if claim.is_some() { None } else { saved.clone() },
        new_id: claim.clone(),
        initial_prompt,
        hook,
        windows: cfg!(windows),
    });
    {
        let store = locked(&store)?;
        if pin {
            settings.account = profile;
            store.save_agent_settings(id, &settings)?;
        }
        if let Some(minted) = launch.minted_id.as_deref()
            && saved.as_deref() != Some(minted)
        {
            store
                .set_terminal_conversation(id, minted)
                .map_err(|error| format!("Could not save the conversation id: {error}"))?;
        }
    }
    let reused_id = saved.is_some()
        && launch
            .minted_id
            .as_deref()
            .is_none_or(|minted| saved.as_deref() == Some(minted));
    Ok(Prepared {
        provider_id: launch.minted_id.clone().or(saved),
        cwd: launch_cwd,
        has_worktree,
        env_set,
        env_remove,
        reused_id,
        claimed: claim.is_some(),
        launch,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Not shown yet: nothing launches until the pane is first shown.
    Idle,
    /// Deciding the launch, off the UI thread.
    Preparing,
    Running,
    Exited,
    /// Killed by the shell; ignores everything after.
    Stopped,
}

pub(crate) struct TuiSession {
    id: i64,
    kind: Kind,
    store: Database,
    /// The project folder.
    project: String,
    /// Where the CLI runs: the worktree unless it was removed.
    cwd: String,
    has_worktree: bool,
    provider_id: Option<String>,
    phase: Phase,
    /// Counts launches; a result for an older one is dropped.
    attempt: u64,
    terminal: Option<Entity<Terminal>>,
    _events: Option<Subscription>,
    launched_at: Option<Instant>,
    /// The launch used an id saved earlier, by resuming or by claiming it.
    reused_id: bool,
    /// This launch is the retry that claims a never-used saved id.
    claimed: bool,
    claim: Option<String>,
    /// The first prompt the CLI starts with (an Inbox item's); sent with the first
    /// launch that gets as far as spawning, never again.
    initial_prompt: Option<String>,
    /// The id already claimed once; a second claim of it would loop.
    claimed_already: Option<String>,
    exit: Option<Exit>,
    busy: bool,
    /// The last title this pane wrote, so the CLI keeps following itself.
    pub(crate) last_cli_title: Option<String>,
    focus_pending: bool,
}
impl EventEmitter<TuiEvent> for TuiSession {}

/// The CLI path the app launches. Resolving probes the filesystem, so tests use
/// a fixed path and never fill the process-wide discovery cache.
fn default_binary(kind: Kind) -> Result<PathBuf, String> {
    if cfg!(test) {
        return Ok(PathBuf::from(format!("/opt/test/{}", kind.id())));
    }
    tui_launch::tui_binary(kind)
}

impl TuiSession {
    pub(crate) fn new(summary: &Session, store: Database, worktree: Option<String>) -> Self {
        Self {
            id: summary.id,
            kind: Kind::from_id(&summary.provider),
            store,
            project: summary.cwd.clone(),
            has_worktree: worktree.is_some(),
            cwd: worktree.unwrap_or_else(|| summary.cwd.clone()),
            provider_id: summary.provider_id.clone(),
            phase: Phase::Idle,
            attempt: 0,
            terminal: None,
            _events: None,
            launched_at: None,
            reused_id: false,
            claimed: false,
            claim: None,
            initial_prompt: None,
            claimed_already: None,
            exit: None,
            busy: false,
            last_cli_title: None,
            focus_pending: false,
        }
    }

    /// The conversation the CLI runs; the id Claude's hooks report.
    pub(crate) fn provider_id(&self) -> Option<&str> {
        self.provider_id.as_deref()
    }
    pub(crate) fn busy(&self) -> bool {
        self.busy
    }
    /// The prompt the first launch starts the CLI with; set before it is shown.
    pub(crate) fn set_initial_prompt(&mut self, prompt: String) {
        self.initial_prompt = Some(prompt).filter(|prompt| !prompt.trim().is_empty());
    }
    #[cfg(test)]
    pub(crate) fn exit(&self) -> Option<&Exit> {
        self.exit.as_ref()
    }
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.phase == Phase::Running
    }
    #[cfg(test)]
    pub(crate) fn stopped(&self) -> bool {
        self.phase == Phase::Stopped
    }
    #[cfg(test)]
    pub(crate) fn terminal(&self) -> Option<&Entity<Terminal>> {
        self.terminal.as_ref()
    }
    /// The bar offers a new conversation: resuming failed at once, or found nothing.
    pub(crate) fn offers_fresh(&self) -> bool {
        self.exit
            .as_ref()
            .is_some_and(|exit| tui_launch::can_start_fresh(self.kind, exit, self.reused_id))
    }

    /// A hook reported a turn starting or ending. Ignored unless the CLI runs,
    /// so a late event cannot mark an exited pane busy.
    pub(crate) fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy == busy || (busy && self.phase != Phase::Running) {
            return;
        }
        self.busy = busy;
        cx.emit(TuiEvent::Busy(busy));
        cx.notify();
    }

    /// Launch on first show; shown again later it does nothing.
    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Idle {
            self.launch(false, cx);
        }
    }
    /// A manual restart starts a new chain, which may claim the id again.
    pub(crate) fn restart(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Exited | Phase::Idle) {
            self.claimed_already = None;
            self.launch(false, cx);
        }
    }
    /// Forget the saved conversation and start a new one. The click is in the
    /// visible pane, so the new process takes the keyboard.
    pub(crate) fn start_fresh(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Exited {
            self.claimed_already = None;
            self.focus_pending = true;
            self.launch(true, cx);
        }
    }
    /// End the CLI. The shell calls this when the pane leaves the workspace or
    /// the session is archived or deleted; nothing relaunches it afterwards.
    pub(crate) fn kill(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Stopped {
            return;
        }
        self.phase = Phase::Stopped;
        self.attempt += 1;
        self._events = None;
        if let Some(terminal) = &self.terminal {
            terminal.read(cx).kill();
        }
        if std::mem::take(&mut self.busy) {
            cx.emit(TuiEvent::Busy(false));
        }
        cx.notify();
    }

    /// The keyboard focus of the running CLI, once there is one.
    pub(crate) fn focus_handle(&self, cx: &gpui::App) -> Option<FocusHandle> {
        self.terminal
            .as_ref()
            .filter(|_| self.phase == Phase::Running)
            .map(|terminal| terminal.read(cx).focus_handle(cx))
    }
    /// Focus the CLI once it runs (not now: it may not exist yet).
    pub(crate) fn focus_when_running(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = true;
        cx.notify();
    }
    pub(crate) fn cancel_focus(&mut self) {
        self.focus_pending = false;
    }
    /// Focus the CLI now, or as soon as it exists.
    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.focus_handle(cx) {
            Some(focus) => window.focus(&focus, cx),
            None => {
                self.focus_pending = true;
                cx.notify();
            }
        }
    }

    fn launch(&mut self, fresh: bool, cx: &mut Context<Self>) {
        self.attempt += 1;
        let attempt = self.attempt;
        // The new process takes the keyboard from a pane that had it (or was about
        // to get it), never from elsewhere: a background pane must not steal it.
        self.focus_pending |= self
            .terminal
            .as_ref()
            .is_some_and(|terminal| terminal.read(cx).has_focus());
        self.phase = Phase::Preparing;
        self.exit = None;
        self.terminal = None;
        self._events = None;
        // A relaunch (claim, restart) ends whatever turn the old process had.
        if std::mem::take(&mut self.busy) {
            cx.emit(TuiEvent::Busy(false));
        }
        let hooks = cx
            .try_global::<crate::settings::Preferences>()
            .is_none_or(|preferences| preferences.claude_hooks);
        let (book, root) = match crate::accounts::get(cx) {
            Some(center) => (center.read(cx).book.clone(), center.read(cx).root.clone()),
            None => (Book::default(), std::env::temp_dir()),
        };
        let request = Request {
            store: self.store.clone(),
            id: self.id,
            kind: self.kind,
            cwd: self.project.clone(),
            provider_id: self.provider_id.clone(),
            claim: self.claim.take(),
            fresh,
            initial_prompt: self.initial_prompt.clone(),
            hooks,
            book,
            root,
            binary: default_binary,
        };
        let work = cx
            .background_executor()
            .spawn(async move { prepare(request) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.prepared(attempt, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn prepared(&mut self, attempt: u64, result: Result<Prepared, String>, cx: &mut Context<Self>) {
        if attempt != self.attempt {
            return;
        }
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                self.settle(
                    Exit {
                        error: Some(error),
                        early: true,
                        ..Default::default()
                    },
                    cx,
                );
                return;
            }
        };
        let Prepared {
            launch,
            cwd,
            has_worktree,
            env_set,
            env_remove,
            provider_id,
            reused_id,
            claimed,
        } = prepared;
        // The prompt rides on this launch; a restart resumes the conversation.
        self.initial_prompt = None;
        if provider_id != self.provider_id {
            self.provider_id = provider_id.clone();
            cx.emit(TuiEvent::Conversation(provider_id));
        }
        self.cwd = cwd;
        self.has_worktree = has_worktree;
        self.reused_id = reused_id;
        self.claimed = claimed;
        self.launched_at = Some(Instant::now());
        let terminal = cx.new(|cx| {
            Terminal::launch(
                Path::new(&self.cwd),
                launch.program,
                launch.args,
                env_set,
                env_remove,
                cx,
            )
        });
        // Right after creating it: the start and spawn-failure events follow a tick later.
        self._events = Some(
            cx.subscribe(&terminal, |this, _, event: &TerminalEvent, cx| {
                this.on_terminal(event.clone(), cx)
            }),
        );
        self.terminal = Some(terminal);
        self.phase = Phase::Running;
        cx.notify();
    }

    fn on_terminal(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::Started { .. } => cx.emit(TuiEvent::Started),
            TerminalEvent::Title(raw) => {
                if let Some(title) = tui_launch::clean_title(&raw) {
                    cx.emit(TuiEvent::Title(title));
                }
            }
            TerminalEvent::Exited { code, error, tail } => self.exited(code, error, &tail, cx),
            TerminalEvent::Changed => cx.notify(),
        }
    }

    fn exited(
        &mut self,
        code: Option<i32>,
        error: Option<String>,
        tail: &str,
        cx: &mut Context<Self>,
    ) {
        let early = self.launched_at.is_none_or(|at| at.elapsed() < EARLY_EXIT);
        let no_conversation = tui_launch::no_conversation(tail);
        // Claude only knows a conversation once it has one, so an untouched tab
        // has a saved id that `--resume` rejects. The id cannot be "in use" if
        // Claude never wrote it, so claim it once before showing an error.
        if error.is_none()
            && early
            && no_conversation
            && self.reused_id
            && !self.claimed
            && let Some(id) = self.provider_id.clone()
            && self.claimed_already.as_deref() != Some(&id)
        {
            self.claimed_already = Some(id.clone());
            self.claim = Some(id);
            self.launch(false, cx);
            return;
        }
        let clean = code == Some(0) && error.is_none() && !early && !no_conversation;
        self.settle(
            Exit {
                code,
                error,
                early,
                no_conversation,
            },
            cx,
        );
        // A failed or early exit keeps the pane so its notice can be read, and
        // so does a late "no conversation" (print mode exits 0 for it).
        if clean {
            cx.emit(TuiEvent::Close);
        }
    }

    /// The pane ends on `exit`; the CLI is no longer busy.
    fn settle(&mut self, exit: Exit, cx: &mut Context<Self>) {
        self.phase = Phase::Exited;
        self.exit = Some(exit.clone());
        if std::mem::take(&mut self.busy) {
            cx.emit(TuiEvent::Busy(false));
        }
        cx.emit(TuiEvent::Exited(exit));
        cx.notify();
    }

    /// Pretend the launch happened `by` earlier, for the early-exit window.
    #[cfg(test)]
    pub(crate) fn age_launch(&mut self, by: std::time::Duration) {
        self.launched_at = self.launched_at.and_then(|at| at.checked_sub(by));
    }
}

/// `~` for the home folder, as the sidebar shows paths.
fn pretty_cwd(cwd: &str) -> String {
    std::env::var_os("HOME")
        .and_then(|home| {
            Path::new(cwd).strip_prefix(home).ok().map(|rest| {
                if rest.as_os_str().is_empty() {
                    "~".to_owned()
                } else {
                    format!("~/{}", rest.display())
                }
            })
        })
        .unwrap_or_else(|| cwd.to_owned())
}

impl TuiSession {
    fn header(&self) -> AnyElement {
        let label = self.kind.label();
        div()
            .debug_selector(|| "tui-header".into())
            .h(px(32.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(crate::right_panel::stroke())
            .text_size(px(12.))
            .text_color(content(0.55))
            .child(provider_icon(self.kind.id(), 14.))
            .child(div().flex_shrink_0().child(format!("{label} terminal")))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(pretty_cwd(&self.cwd)),
            )
            // A terminal session runs in the current checkout.
            .when(!self.has_worktree, |d| {
                d.child(
                    div()
                        .id("tui-new-worktree")
                        .debug_selector(|| "tui-new-worktree".into())
                        .role(Role::Button)
                        .aria_label("New worktree is not available in terminal sessions")
                        .tooltip(|_, cx| cx.new(|_| Tip(NEW_WORKTREE_TIP)).into())
                        .flex_shrink_0()
                        .h(px(24.))
                        .px_1p5()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .rounded_md()
                        .opacity(0.4)
                        .child(icon("folder-tree", 14.))
                        .child("New worktree unavailable"),
                )
            })
            .into_any_element()
    }

    fn bar(&self, exit: &Exit, cx: &mut Context<Self>) -> AnyElement {
        let notice = tui_launch::exit_notice(self.kind, exit);
        let fresh = self.offers_fresh();
        let button = |id: &'static str, label: &'static str, strong: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .role(Role::Button)
                .aria_label(label)
                .flex_shrink_0()
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_color(content(if strong { 1. } else { 0.65 }))
                .when(strong, |d| d.bg(content(0.08)))
                .hover(|s| s.bg(content(0.12)).text_color(content(1.)))
                .child(label)
        };
        div()
            .debug_selector(|| "tui-exit-bar".into())
            .flex_shrink_0()
            .flex()
            .items_start()
            .gap_3()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(crate::right_panel::stroke())
            .text_size(px(12.))
            .text_color(content(0.8))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(notice.message),
                    )
                    .children(
                        notice
                            .hint
                            .map(|hint| div().mt_0p5().text_color(content(0.55)).child(hint)),
                    ),
            )
            .when(fresh, |d| {
                d.child(
                    button("tui-start-fresh", "Start new conversation", false)
                        .on_click(cx.listener(|this, _, _, cx| this.start_fresh(cx))),
                )
            })
            .child(
                button("tui-restart", "Restart", true)
                    .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
            )
            .into_any_element()
    }
}

impl Render for TuiSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending
            && let Some(focus) = self.focus_handle(cx)
        {
            self.focus_pending = false;
            window.focus(&focus, cx);
        }
        let body = match &self.terminal {
            Some(terminal) => div().size_full().child(
                terminal
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
            None if self.exit.is_none() => div()
                .p_3()
                .text_size(px(12.))
                .text_color(content(0.45))
                .child(format!("Starting {}...", self.kind.label())),
            None => div(),
        };
        let bar = self.exit.clone().map(|exit| self.bar(&exit, cx));
        div()
            .id(("tui-session", self.id as usize))
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.header())
            .child(div().relative().min_h_0().min_w_0().flex_1().child(body))
            .children(bar)
    }
}
