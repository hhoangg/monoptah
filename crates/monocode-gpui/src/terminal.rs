//! Port of TerminalView.tsx: a login shell on a PTY, emulated by
//! alacritty_terminal and painted as a grid of cell-snapped text runs.
use crate::theme::*;
use alacritty_terminal::{
    event::{Event as TermEvent, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    index::{Boundary, Column, Direction, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{
        Config, Term, TermMode,
        cell::Flags,
        color::Colors,
        point_to_viewport,
        search::{RegexIter, RegexSearch},
        viewport_to_point,
    },
    tty,
    vte::ansi::{Color as AnsiColor, CursorShape, CursorStyle as TermCursorStyle, NamedColor, Rgb},
};
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight,
    GlobalElementId, Hsla, InputHandler, InspectorElementId, KeyBinding, KeyDownEvent, Keystroke,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, ScrollDelta, ScrollWheelEvent,
    ShapedLine, SharedString, Size, StrikethroughStyle, Style, Task, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, actions, div, point, prelude::*, px, rgb, size,
};
use std::{
    borrow::Cow,
    cell::Cell,
    collections::HashMap,
    ffi::OsString,
    ops::{Range, RangeInclusive},
    path::Path,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

actions!(
    terminal,
    [SendTab, SendBackTab, SendEscape, Copy, Paste, SelectAll]
);

pub(crate) fn bind(cx: &mut App) {
    let m = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl-shift"
    };
    let c = Some("Terminal");
    // The shell owns these keys while a terminal has focus.
    cx.bind_keys([
        KeyBinding::new("tab", SendTab, c),
        KeyBinding::new("shift-tab", SendBackTab, c),
        KeyBinding::new("escape", SendEscape, c),
        KeyBinding::new(&format!("{m}-c"), Copy, c),
        KeyBinding::new(&format!("{m}-v"), Paste, c),
        KeyBinding::new(&format!("{m}-a"), SelectAll, c),
    ]);
}

const FONT_SIZE: f32 = 13.;
/// TerminalView's `scrollback`, retained for every open terminal.
const SCROLLBACK: usize = 2000;
const PADDING: f32 = 8.;
/// The web build's WebLinksAddon URL pattern.
const URL_PATTERN: &str = r#"(https?|HTTPS?)://[^\s"'!*(){}|\\^<>`]*[^\s"':,.!?{}|\\^~\[\]`()<>]"#;

/// Wakeups, titles and replies from the PTY thread.
#[derive(Clone)]
struct Listener(async_channel::Sender<TermEvent>);
impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.try_send(event);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GridSize {
    columns: usize,
    lines: usize,
}
impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TerminalEvent {
    /// Title, foreground process or running state changed.
    Changed,
    /// A launched program is running. Only `Terminal::launch` emits the rest,
    /// from the event task, so subscribe right after creating the entity.
    Started { at: Instant },
    /// The launched program ended, or never started (`error`, no `code`).
    /// `tail` is the last `TAIL_CHARS` of grid text as the user saw it,
    /// without the `[process exited]` note.
    Exited {
        code: Option<i32>,
        error: Option<String>,
        tail: String,
    },
    /// The CLI set its window title (raw, not cleaned); repeats are dropped.
    Title(String),
}

/// Exit tails and `Terminal::text` keep this many trailing characters.
const TAIL_CHARS: usize = 4000;
/// A program that ignores the hangup gets SIGKILL after this long.
const KILL_ESCALATE: Duration = Duration::from_secs(1);

/// Where the grid was painted, for mapping the pointer to cells.
#[derive(Clone, Copy, Default)]
struct Geometry {
    origin: gpui::Point<gpui::Pixels>,
    cell: Size<gpui::Pixels>,
}

/// An HTTP(S) link in the grid: an OSC 8 hyperlink or a plain URL.
#[derive(Clone, PartialEq, Debug)]
struct Link {
    range: RangeInclusive<GridPoint>,
    uri: String,
}

/// Text an input method is composing. It lives only here until the method
/// commits it: the PTY never sees marked text.
#[derive(Clone, Debug, PartialEq)]
struct Marked {
    text: String,
    /// The caret or selection inside `text`, in UTF-16 units.
    selected: Range<usize>,
}

pub(crate) struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: Option<EventLoopSender>,
    focus: FocusHandle,
    pub cwd: String,
    /// The tab label: the foreground command, else the directory.
    pub title: String,
    /// A command other than the shell holds the terminal.
    pub foreground: Option<String>,
    pub exited: bool,
    size: GridSize,
    geometry: Rc<Cell<Option<Geometry>>>,
    master_fd: i32,
    shell_pid: u32,
    /// Created by `launch`: reports start, title and exit instead of a shell prompt.
    launched: bool,
    /// When the launched program started.
    pub started_at: Option<Instant>,
    spawn_error: Option<String>,
    /// `ChildExit` waits for `Exit`, which follows the final output drain.
    child_code: Option<Option<i32>>,
    last_title: Option<String>,
    /// Lets `process::shutdown_all` reap the launched program on quit.
    registration: Option<monocode_core::process::Registration>,
    #[cfg(test)]
    launch_spec: Option<LaunchSpec>,
    selecting: bool,
    selection_pointer: Option<gpui::Point<gpui::Pixels>>,
    selection_scroll: Option<Task<()>>,
    scroll_remainder: f32,
    url_regex: Option<RegexSearch>,
    /// The pointer over the grid, for underlining the link beneath it.
    hover: Option<gpui::Point<gpui::Pixels>>,
    /// The link a plain click pressed; releasing on it opens it.
    pressed_link: Option<Link>,
    pub(crate) visible: bool,
    /// Has the keyboard (tracked from the first draw); a relaunch hands it on.
    focused: bool,
    /// Text the input method is composing, shown at the cursor.
    marked: Option<Marked>,
    /// Every byte `write` was asked to send, since a test PTY has no reader.
    #[cfg(test)]
    sent: std::cell::RefCell<Vec<u8>>,
    _focus_watch: Vec<gpui::Subscription>,
    _events: Task<()>,
    /// The dock's foreground poll; a launched program has no shell to watch.
    _poll: Option<Task<()>>,
}
impl EventEmitter<TerminalEvent> for Terminal {}
impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

struct Spawned {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: Option<EventLoopSender>,
    master_fd: i32,
    shell_pid: u32,
    events: async_channel::Receiver<TermEvent>,
    /// Why the program could not start (also written into the grid).
    error: Option<String>,
}
/// What runs on the PTY: the login shell, or a program launched by a session.
struct Plan {
    program: String,
    args: Vec<String>,
    /// Set on top of the base environment.
    env: Vec<(String, String)>,
    /// Read the output left in the PTY after the child exits.
    drain: bool,
    /// Names the program in the "could not start" note.
    label: String,
    /// The real program when it runs under the `env` wrapper: `env` would
    /// start fine and then exit 127, so it is checked before spawning.
    verify: Option<String>,
}
impl Plan {
    fn shell() -> Self {
        let (program, args) = default_shell();
        Self {
            program,
            args,
            env: Vec::new(),
            drain: false,
            label: "the shell".into(),
            verify: None,
        }
    }
    fn launch(
        program: OsString,
        args: Vec<OsString>,
        env: Vec<(String, OsString)>,
        remove: &[&str],
    ) -> Self {
        let label = program.to_string_lossy().into_owned();
        let verify = (cfg!(unix) && !remove.is_empty()).then(|| label.clone());
        let (program, args) = wrap_removals(program, args, remove);
        Self {
            program: lossy(program),
            args: args.into_iter().map(lossy).collect(),
            env: env.into_iter().map(|(k, v)| (k, lossy(v))).collect(),
            // The last words a CLI prints before it dies are the exit tail.
            drain: true,
            label,
            verify,
        }
    }
}
fn lossy(value: OsString) -> String {
    value
        .into_string()
        .unwrap_or_else(|value| value.to_string_lossy().into_owned())
}
/// alacritty's `tty::Options` cannot remove a variable, so the program runs
/// under `env -u KEY -- program` (which then execs it, keeping the pid).
/// Windows only sets. A program name containing `=` cannot be expressed through
/// `env`; resolved CLI paths never contain one.
fn wrap_removals(
    program: OsString,
    args: Vec<OsString>,
    remove: &[&str],
) -> (OsString, Vec<OsString>) {
    if !cfg!(unix) || remove.is_empty() {
        return (program, args);
    }
    let mut wrapped = Vec::with_capacity(remove.len() * 2 + args.len() + 1);
    for key in remove {
        wrapped.push(OsString::from("-u"));
        wrapped.push(OsString::from(key));
    }
    wrapped.push(OsString::from("--"));
    wrapped.push(program);
    wrapped.extend(args);
    (OsString::from("/usr/bin/env"), wrapped)
}
/// What `execvp` would say about `program`: not found on the child's PATH (or
/// at the given path), or found but not executable.
#[cfg(unix)]
fn program_problem(program: &str, path: &str, workdir: &Path) -> Option<std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let candidates: Vec<std::path::PathBuf> = if program.contains('/') {
        vec![workdir.join(program)]
    } else {
        std::env::split_paths(path)
            .map(|dir| dir.join(program))
            .collect()
    };
    let mut denied = false;
    for candidate in candidates {
        if let Ok(meta) = std::fs::metadata(&candidate)
            && meta.is_file()
        {
            if meta.permissions().mode() & 0o111 != 0 {
                return None;
            }
            denied = true;
        }
    }
    Some(std::io::Error::from_raw_os_error(if denied {
        libc::EACCES
    } else {
        libc::ENOENT
    }))
}
#[cfg(not(unix))]
fn program_problem(_: &str, _: &str, _: &Path) -> Option<std::io::Error> {
    None
}
/// Start `plan` on a PTY with TerminalView's environment. A failure is
/// written into the grid, as TerminalView prints it.
fn spawn(cwd: &str, size: GridSize, id: u64, plan: Plan) -> Spawned {
    let (tx, rx) = async_channel::unbounded();
    let listener = Listener(tx);
    let config = Config {
        scrolling_history: SCROLLBACK,
        default_cursor_style: TermCursorStyle {
            shape: CursorShape::Beam,
            blinking: true,
        },
        ..Config::default()
    };
    let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
    let workdir = working_dir(cwd);
    let mut env = HashMap::new();
    for (key, value) in [
        ("TERM", "xterm-256color".to_owned()),
        ("COLORTERM", "truecolor".to_owned()),
        ("COLORFGBG", "15;0".to_owned()),
        ("TERM_PROGRAM", "Monoptah".to_owned()),
        ("PATH", monocode_core::scm::terminal_search_path()),
        ("PWD", workdir.to_string_lossy().into_owned()),
    ] {
        env.insert(key.to_owned(), value);
    }
    env.extend(plan.env);
    let problem = plan
        .verify
        .as_deref()
        .and_then(|program| program_problem(program, &env["PATH"], &workdir));
    let options = tty::Options {
        shell: Some(tty::Shell::new(plan.program, plan.args)),
        working_directory: Some(workdir),
        drain_on_exit: false,
        env,
        #[cfg(target_os = "windows")]
        escape_args: true,
    };
    let window = WindowSize {
        num_lines: size.lines as u16,
        num_cols: size.columns as u16,
        cell_width: 8,
        cell_height: 16,
    };
    let mut master_fd = -1;
    let mut shell_pid = 0;
    let mut failure = None;
    let started = match problem {
        Some(error) => Err(error),
        None => tty::new(&options, window, id),
    };
    let sender = match started {
        Ok(pty) => {
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                master_fd = pty.file().as_raw_fd();
                shell_pid = pty.child().id();
            }
            match EventLoop::new(term.clone(), listener, pty, plan.drain, false) {
                Ok(event_loop) => {
                    let sender = event_loop.channel();
                    event_loop.spawn();
                    Some(sender)
                }
                Err(error) => {
                    write_note(&term, &format!("\x1b[31m{error}\x1b[0m"));
                    failure = Some(error.to_string());
                    None
                }
            }
        }
        Err(error) => {
            write_note(
                &term,
                &format!("\x1b[31mCould not start {}: {error}\x1b[0m", plan.label),
            );
            failure = Some(error.to_string());
            None
        }
    };
    Spawned {
        term,
        sender,
        master_fd,
        shell_pid,
        events: rx,
        error: failure,
    }
}

fn detached(size: GridSize) -> Spawned {
    let (tx, rx) = async_channel::unbounded();
    let term = Term::new(Config::default(), &size, Listener(tx));
    Spawned {
        term: Arc::new(FairMutex::new(term)),
        sender: None,
        master_fd: -1,
        shell_pid: 0,
        events: rx,
        error: None,
    }
}

/// What `Terminal::launch` was asked to run, kept so tests can assert on it.
#[cfg(test)]
pub(crate) struct LaunchSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env_set: Vec<(String, OsString)>,
    pub env_remove: Vec<&'static str>,
}

const GRID: GridSize = GridSize {
    columns: 80,
    lines: 24,
};

impl Terminal {
    pub(crate) fn new(cwd: String, cx: &mut Context<Self>) -> Self {
        let spawned = if cfg!(test) {
            // GPUI's deterministic test scheduler rejects wakeups from the
            // PTY thread; UI tests run the dock without a shell behind it.
            detached(GRID)
        } else {
            spawn(&cwd, GRID, cx.entity_id().as_u64(), Plan::shell())
        };
        Self::build(cwd, spawned, false, cx)
    }

    /// Run `program` instead of the login shell, with the dock's base
    /// environment. `env_set` is added and `env_remove` removed (on unix by an
    /// `env -u` wrapper). The directory falls back to home when missing.
    /// Reports `Started`, `Title` and `Exited`; tests fake these with the
    /// `fake_*` methods since GPUI tests cannot run a PTY.
    pub(crate) fn launch(
        cwd: &Path,
        program: OsString,
        args: Vec<OsString>,
        env_set: Vec<(String, OsString)>,
        env_remove: Vec<&'static str>,
        cx: &mut Context<Self>,
    ) -> Self {
        #[cfg(test)]
        let spec = LaunchSpec {
            program: program.clone(),
            args: args.clone(),
            env_set: env_set.clone(),
            env_remove: env_remove.clone(),
        };
        let cwd = cwd.to_string_lossy().into_owned();
        let plan = Plan::launch(program, args, env_set, &env_remove);
        let spawned = if cfg!(test) {
            detached(GRID)
        } else {
            spawn(&cwd, GRID, cx.entity_id().as_u64(), plan)
        };
        let mut terminal = Self::build(cwd, spawned, true, cx);
        if terminal.shell_pid > 1 {
            terminal.registration = Some(monocode_core::process::track(terminal.shell_pid));
        }
        #[cfg(test)]
        {
            terminal.launch_spec = Some(spec);
        }
        terminal
    }

    fn build(cwd: String, spawned: Spawned, launched: bool, cx: &mut Context<Self>) -> Self {
        let size = GRID;
        let Spawned {
            term,
            sender,
            master_fd,
            shell_pid,
            events: rx,
            error,
        } = spawned;
        let events = cx.spawn(async move |this, cx| {
            // Not in the constructor: nobody has subscribed yet.
            if launched && !cfg!(test) {
                let _ = this.update(cx, |this, cx| this.announce_launch(cx));
            }
            while let Ok(event) = rx.recv().await {
                // Output arrives in bursts; one repaint covers the burst.
                let mut batch = vec![event];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |this, cx| {
                        for event in batch {
                            this.on_event(event, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        // TerminalView's 1s foreground poll; a hidden dock has no title to paint.
        let poll = (!launched).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let Ok(Some((fd, pid))) = this.update(cx, |this, _| {
                        (this.visible && !this.exited).then_some((this.master_fd, this.shell_pid))
                    }) else {
                        if this.update(cx, |_, _| ()).is_err() {
                            break;
                        }
                        continue;
                    };
                    let label = cx
                        .background_executor()
                        .spawn(async move { foreground_label(fd, pid) })
                        .await;
                    if this
                        .update(cx, |this, cx| this.set_foreground(label, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        Self {
            term,
            sender,
            focus: cx.focus_handle(),
            title: default_title(&cwd),
            cwd,
            foreground: None,
            exited: false,
            size,
            geometry: Rc::default(),
            master_fd,
            shell_pid,
            launched,
            started_at: None,
            spawn_error: error,
            child_code: None,
            last_title: None,
            registration: None,
            #[cfg(test)]
            launch_spec: None,
            selecting: false,
            selection_pointer: None,
            selection_scroll: None,
            scroll_remainder: 0.,
            url_regex: RegexSearch::new(URL_PATTERN).ok(),
            hover: None,
            pressed_link: None,
            visible: true,
            focused: false,
            marked: None,
            #[cfg(test)]
            sent: Default::default(),
            _focus_watch: Vec::new(),
            _events: events,
            _poll: poll,
        }
    }

    fn on_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {
                cx.notify()
            }
            TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
            TermEvent::ColorRequest(index, format) => {
                let colors = *self.term.lock().colors();
                self.write(format(color_rgb(index, &colors)).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let cell = self
                    .geometry
                    .get()
                    .map_or(size(px(8.), px(16.)), |g| g.cell);
                let reply = format(WindowSize {
                    num_lines: self.size.lines as u16,
                    num_cols: self.size.columns as u16,
                    cell_width: cell.width.as_f32() as u16,
                    cell_height: cell.height.as_f32() as u16,
                });
                self.write(reply.into_bytes());
            }
            TermEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            TermEvent::ClipboardLoad(_, format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.write(format(&text).into_bytes());
            }
            TermEvent::ChildExit(status) => self.child_exited(status.code(), cx),
            TermEvent::Exit if self.launched => {
                let code = self.child_code.take().flatten();
                self.finish_launch(code, None, cx);
            }
            TermEvent::Exit => self.finish(cx),
            TermEvent::Title(title) if self.launched => {
                if self.last_title.as_deref() != Some(&title) {
                    self.last_title = Some(title.clone());
                    cx.emit(TerminalEvent::Title(title));
                }
            }
            TermEvent::Title(_) | TermEvent::ResetTitle | TermEvent::Bell => {}
        }
    }
    fn child_exited(&mut self, code: Option<i32>, cx: &mut Context<Self>) {
        if self.launched {
            // The output still in the PTY is read before `Exit` follows; the
            // tail and the note wait for it.
            self.child_code = Some(code);
        } else {
            write_note(&self.term, &Self::exit_note(code));
            self.finish(cx);
        }
    }
    fn exit_note(code: Option<i32>) -> String {
        let code = code.map(|c| format!(" ({c})")).unwrap_or_default();
        format!("\r\n[process exited{code}]")
    }
    fn announce_launch(&mut self, cx: &mut Context<Self>) {
        match self.spawn_error.take() {
            Some(error) => self.finish_launch(None, Some(error), cx),
            None => self.start(cx),
        }
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        let at = Instant::now();
        self.started_at = Some(at);
        cx.emit(TerminalEvent::Started { at });
    }
    /// A launched program ended or never started: report it once.
    fn finish_launch(&mut self, code: Option<i32>, error: Option<String>, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let tail = tail(&grid_text(&self.term.lock()));
        if error.is_none() {
            write_note(&self.term, &Self::exit_note(code));
        }
        self.finish(cx);
        cx.emit(TerminalEvent::Exited { code, error, tail });
    }
    /// The keyboard is here (tracked from the first draw).
    pub(crate) fn has_focus(&self) -> bool {
        self.focused
    }
    /// The grid as plain text (history included), last `TAIL_CHARS` characters.
    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        tail(&grid_text(&self.term.lock()))
    }
    /// SIGHUP and SIGTERM to the program and its group, SIGKILL a second later
    /// if it is still there. Never blocks; `Exited` follows when it dies.
    pub(crate) fn kill(&self) {
        if !self.exited {
            terminate(self.shell_pid);
        }
    }
    fn finish(&mut self, cx: &mut Context<Self>) {
        if !self.exited {
            self.exited = true;
            self.sender = None;
            self.registration = None;
            self.set_foreground(None, cx);
            cx.emit(TerminalEvent::Changed);
        }
        cx.notify();
    }
    fn set_foreground(&mut self, label: Option<String>, cx: &mut Context<Self>) {
        if label == self.foreground {
            return;
        }
        self.title = label.clone().unwrap_or_else(|| default_title(&self.cwd));
        self.foreground = label;
        cx.emit(TerminalEvent::Changed);
    }
    /// Hangs up the job holding the terminal, leaving the shell at its prompt.
    pub(crate) fn stop_foreground(&mut self, cx: &mut Context<Self>) {
        #[cfg(unix)]
        if self.master_fd >= 0 && !self.exited {
            let pgrp = unsafe { libc::tcgetpgrp(self.master_fd) };
            if pgrp > 1 && pgrp != self.shell_pid as i32 {
                unsafe {
                    libc::kill(-pgrp, libc::SIGHUP);
                }
            }
        }
        self.set_foreground(None, cx);
    }
    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        #[cfg(test)]
        self.sent.borrow_mut().extend_from_slice(&bytes);
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Input(bytes));
        }
    }
    /// Typing returns a scrolled-back view to the prompt.
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.stop_selection();
        {
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
        cx.notify();
    }
    /// Types `command` at the prompt and runs it. The footer names it at
    /// once; the foreground poll corrects the label within a second.
    pub(crate) fn run(&mut self, command: &str, cx: &mut Context<Self>) {
        let mut line = command.trim().replace("\r\n", "\r").replace('\n', "\r");
        line.push('\r');
        self.input(line.into_bytes(), cx);
        if let Some(label) = command_label(command.trim()) {
            self.set_foreground(Some(label), cx);
        }
    }
    /// Fit the grid to the painted area; the PTY learns its new size.
    fn resize(&mut self, columns: usize, lines: usize, cell: Size<gpui::Pixels>) {
        let next = GridSize { columns, lines };
        if next == self.size {
            return;
        }
        self.size = next;
        self.term.lock().resize(next);
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Resize(WindowSize {
                num_lines: lines as u16,
                num_cols: columns as u16,
                cell_width: cell.width.as_f32() as u16,
                cell_height: cell.height.as_f32() as u16,
            }));
        }
    }

    /// Special keys and shortcuts. Typed text is left unhandled so the platform
    /// delivers it through the input handler: an input method that rewrites the
    /// text (Vietnamese, Telex, kana) sends a key whose code names a different
    /// letter than the characters it commits.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let mode = *self.term.lock().mode();
        if let Some(bytes) = key_bytes(&event.keystroke, mode) {
            cx.stop_propagation();
            self.input(bytes, cx);
        }
    }
    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self
            .term
            .lock()
            .selection_to_string()
            .filter(|t| !t.is_empty())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let bytes = if bracketed {
            // A pasted end marker must not end the paste early.
            let text = text.replace("\x1b[201~", "");
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.input(bytes, cx);
    }
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        let mut selection = Selection::new(
            SelectionType::Simple,
            GridPoint::new(term.topmost_line(), Column(0)),
            Side::Left,
        );
        selection.update(
            GridPoint::new(term.bottommost_line(), term.last_column()),
            Side::Right,
        );
        term.selection = Some(selection);
        drop(term);
        cx.notify();
    }

    /// The cell under a window position, and which half of it.
    /// The zero-based viewport line and column under a pointer.
    fn viewport_cell(&self, position: gpui::Point<gpui::Pixels>) -> (usize, usize) {
        let Some(g) = self.geometry.get() else {
            return (0, 0);
        };
        let x = ((position.x - g.origin.x) / g.cell.width).max(0.);
        let y = ((position.y - g.origin.y) / g.cell.height).max(0.);
        (
            (y as usize).min(self.size.lines.saturating_sub(1)),
            (x as usize).min(self.size.columns.saturating_sub(1)),
        )
    }
    fn grid_point(&self, position: gpui::Point<gpui::Pixels>) -> Option<(GridPoint, Side)> {
        let g = self.geometry.get()?;
        let x = ((position.x - g.origin.x) / g.cell.width).max(0.);
        let y = ((position.y - g.origin.y) / g.cell.height).max(0.);
        let column = (x as usize).min(self.size.columns.saturating_sub(1));
        let line = (y as usize).min(self.size.lines.saturating_sub(1));
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let offset = self.term.lock().grid().display_offset();
        Some((
            viewport_to_point(offset, GridPoint::new(line, Column(column))),
            side,
        ))
    }
    /// The HTTP(S) link under a window position, if the pointer is on a cell.
    fn link_at(&mut self, position: gpui::Point<gpui::Pixels>) -> Option<Link> {
        let g = self.geometry.get()?;
        let x = (position.x - g.origin.x) / g.cell.width;
        let y = (position.y - g.origin.y) / g.cell.height;
        if x < 0. || y < 0. || x >= self.size.columns as f32 || y >= self.size.lines as f32 {
            return None;
        }
        let term = self.term.lock();
        let at = viewport_to_point(
            term.grid().display_offset(),
            GridPoint::new(y as usize, Column(x as usize)),
        );
        link_at(&term, self.url_regex.as_mut()?, at)
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.pressed_link = (event.click_count == 1 && !event.modifiers.shift)
            .then(|| self.link_at(event.position))
            .flatten();
        let Some((at, side)) = self.grid_point(event.position) else {
            return;
        };
        let mut term = self.term.lock();
        match term.selection.as_mut() {
            Some(selection) if event.modifiers.shift => selection.update(at, side),
            _ => {
                let ty = match event.click_count {
                    2 => SelectionType::Semantic,
                    3.. => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                term.selection = Some(Selection::new(ty, at, side));
            }
        }
        drop(term);
        self.selecting = true;
        self.selection_pointer = None;
        self.selection_scroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if !this
                    .update_in(cx, |this, window, cx| {
                        if !window.is_window_active() || !this.focus.is_focused(window) {
                            this.selecting = false;
                            this.selection_pointer = None;
                            return false;
                        }
                        this.scroll_selection(cx)
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        }));
        cx.notify();
    }
    /// A click that pressed and released on the same link, without
    /// dragging out a selection, opens it as the web build's xterm does.
    fn mouse_up(&mut self, event: &gpui::MouseUpEvent, cx: &mut Context<Self>) {
        self.stop_selection();
        let Some(link) = self.pressed_link.take() else {
            return;
        };
        let selected = self
            .term
            .lock()
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.is_empty());
        if !selected && self.link_at(event.position).as_ref() == Some(&link) {
            cx.open_url(&link.uri);
        }
    }
    fn stop_selection(&mut self) {
        self.selecting = false;
        self.selection_pointer = None;
        self.selection_scroll = None;
    }
    /// Keep extending the same selection through scrollback at either edge,
    /// even when the pointer is held still outside the terminal.
    fn scroll_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.selecting || !self.visible {
            self.selecting = false;
            self.selection_pointer = None;
            return false;
        }
        let Some(position) = self.selection_pointer else {
            // A press without a drag must not scroll a first/last-row click.
            return true;
        };
        let Some(geometry) = self.geometry.get() else {
            return true;
        };
        let y = (position.y - geometry.origin.y) / geometry.cell.height;
        let delta = if y < 0.5 {
            (0.5 - y).ceil().clamp(1., 10.) as i32
        } else if y > self.size.lines as f32 - 0.5 {
            -((y - self.size.lines as f32 + 0.5).ceil().clamp(1., 10.) as i32)
        } else {
            return true;
        };
        let changed = {
            let mut term = self.term.lock();
            if term.selection.is_none() {
                return false;
            }
            let before = term.grid().display_offset();
            term.scroll_display(Scroll::Delta(delta));
            term.grid().display_offset() != before
        };
        if changed {
            if let Some((at, side)) = self.grid_point(position)
                && let Some(selection) = self.term.lock().selection.as_mut()
            {
                selection.update(at, side);
            }
            cx.notify();
        }
        true
    }
    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.stop_selection();
            return;
        }
        self.selection_pointer = Some(event.position);
        if let Some((at, side)) = self.grid_point(event.position) {
            if let Some(selection) = self.term.lock().selection.as_mut() {
                selection.update(at, side);
            }
            cx.notify();
        }
    }
    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let line = self.geometry.get().map_or(16., |g| g.cell.height.as_f32());
        let delta = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => pixels.y.as_f32() / line,
        } + self.scroll_remainder;
        let lines = delta.trunc() as i32;
        self.scroll_remainder = delta.fract();
        if lines == 0 {
            return;
        }
        let mode = *self.term.lock().mode();
        // A program tracking the mouse gets the wheel itself.
        if mode.intersects(TermMode::MOUSE_MODE) {
            let (line, column) = self.viewport_cell(event.position);
            let report = wheel_report(lines > 0, line, column, mode);
            self.write(report.repeat(lines.unsigned_abs() as usize));
        // A full-screen program scrolls itself; the shell scrolls history.
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(key.repeat(lines.unsigned_abs() as usize));
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }
}
/// The spawner seam: a launched terminal in a GPUI test has no process, and
/// these play what the PTY thread and the child would do, through the real
/// handlers.
#[cfg(test)]
impl Terminal {
    pub(crate) fn launched(&self) -> Option<&LaunchSpec> {
        self.launch_spec.as_ref()
    }
    /// The program started.
    pub(crate) fn fake_started(&mut self, cx: &mut Context<Self>) {
        self.start(cx);
    }
    /// The program printed `text` (ANSI and CRLF as a PTY would send them).
    pub(crate) fn fake_output(&mut self, text: &str, cx: &mut Context<Self>) {
        write_note(&self.term, text);
        cx.notify();
    }
    pub(crate) fn fake_title(&mut self, title: &str, cx: &mut Context<Self>) {
        self.on_event(TermEvent::Title(title.into()), cx);
    }
    /// The program ended (`None`: killed by a signal), as `ChildExit` then `Exit`.
    pub(crate) fn fake_exit(&mut self, code: Option<i32>, cx: &mut Context<Self>) {
        self.child_exited(code, cx);
        self.on_event(TermEvent::Exit, cx);
    }
    /// The program could not be started.
    pub(crate) fn fake_spawn_error(&mut self, error: &str, cx: &mut Context<Self>) {
        let program = self
            .launch_spec
            .as_ref()
            .map(|spec| spec.program.to_string_lossy().into_owned())
            .unwrap_or_default();
        write_note(&self.term, &format!("Could not start {program}: {error}"));
        self.spawn_error = Some(error.to_owned());
        self.announce_launch(cx);
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Msg::Shutdown);
        }
        if self.launched {
            self.kill();
            return;
        }
        // The hangup a closing shell expects, for it and its job.
        #[cfg(unix)]
        if self.shell_pid > 1 && !self.exited {
            let pid = self.shell_pid as i32;
            unsafe {
                libc::kill(pid, libc::SIGHUP);
                libc::kill(-pid, libc::SIGHUP);
            }
        }
    }
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        self.focused = focused;
        // Tracked from the first draw on, so `has_focus` is right even while this
        // view is cached and not drawn again.
        if self._focus_watch.is_empty() {
            let focus = self.focus.clone();
            self._focus_watch = vec![
                cx.on_focus_in(&focus, window, |this, _, cx| {
                    this.focused = true;
                    cx.notify();
                }),
                cx.on_blur(&focus, window, |this, _, cx| {
                    this.focused = false;
                    // The input method drops its composition with focus.
                    this.marked = None;
                    cx.notify();
                }),
            ];
        }
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .cursor(gpui::CursorStyle::IBeam)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.hover != Some(event.position) {
                    this.hover = Some(event.position);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_action(cx.listener(|this, _: &SendTab, _, cx| this.input(b"\t".to_vec(), cx)))
            .on_action(
                cx.listener(|this, _: &SendBackTab, _, cx| this.input(b"\x1b[Z".to_vec(), cx)),
            )
            .on_action(cx.listener(|this, _: &SendEscape, _, cx| this.input(b"\x1b".to_vec(), cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.mouse_down(event, window, cx)),
            )
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.scroll(event, cx)))
            .child(Grid {
                terminal: cx.entity(),
                focused,
            })
    }
}

/// Text from the platform's input system: typed characters, and what an input
/// method commits. The document it edits is only the marked text: the PTY owns
/// everything already sent, so nothing before the cursor can be revised.
impl EntityInputHandler for Terminal {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let marked = self.marked.as_ref()?;
        let (text, adjusted) = utf16_slice(&marked.text, range);
        *adjusted_range = Some(adjusted);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self
                .marked
                .as_ref()
                .map_or(0..0, |marked| marked.selected.clone()),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|marked| 0..marked.text.encode_utf16().count())
    }

    /// The method accepted its composition as it stands.
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(marked) = self.marked.take() {
            self.commit(&marked.text, cx);
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Committed text replaces the composition (and nothing else: the
        // range is in terms of the marked text, which is dropped here).
        self.marked = None;
        self.commit(text, cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| {
            let len = new_text.encode_utf16().count();
            let selected = new_selected_range
                .map_or(len..len, |range| range.start.min(len)..range.end.min(len));
            Marked {
                text: new_text.to_owned(),
                selected,
            }
        });
        cx.notify();
    }

    /// The cursor cell, so the candidate window opens where the text goes.
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<gpui::Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<gpui::Pixels>> {
        let geometry = self.geometry.get()?;
        let at = {
            let term = self.term.lock();
            point_to_viewport(term.grid().display_offset(), term.grid().cursor.point)?
        };
        Some(cell_rect(
            geometry.origin,
            geometry.cell,
            at.line,
            at.column.0,
            at.column.0 + 1,
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<gpui::Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Terminal {
    /// Sends text the platform finished composing to the program.
    fn commit(&mut self, text: &str, cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.input(text.as_bytes().to_vec(), cx);
        }
    }
}

/// The part of `text` inside `range` (UTF-16 units), widened to whole
/// characters, and the range it actually covers.
fn utf16_slice(text: &str, range: Range<usize>) -> (String, Range<usize>) {
    let mut out = String::new();
    let mut covered: Option<Range<usize>> = None;
    let mut at = 0;
    for c in text.chars() {
        let end = at + c.len_utf16();
        if end > range.start && at < range.end {
            out.push(c);
            covered = Some(covered.map_or(at..end, |covered| covered.start..end));
        }
        at = end;
    }
    let covered = covered.unwrap_or_else(|| {
        let edge = range.start.min(at);
        edge..edge
    });
    (out, covered)
}

/// `ElementInputHandler` for the terminal, except that a held key keeps
/// repeating. The default hands repeats to macOS, whose press-and-hold accent
/// menu swallows them for letters, which a shell or editor must not lose.
struct TerminalInputHandler(ElementInputHandler<Terminal>);
impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.0
            .selected_text_range(ignore_disabled_input, window, cx)
    }
    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.0.marked_text_range(window, cx)
    }
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.0
            .text_for_range(range_utf16, adjusted_range, window, cx)
    }
    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0
            .replace_text_in_range(replacement_range, text, window, cx)
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0
            .replace_and_mark_text_in_range(range_utf16, new_text, new_selected_range, window, cx)
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.0.unmark_text(window, cx)
    }
    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut App) {
        self.0.paste(item, window, cx)
    }
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<gpui::Pixels>> {
        self.0.bounds_for_range(range_utf16, window, cx)
    }
    fn character_index_for_point(
        &mut self,
        point: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.0.character_index_for_point(point, window, cx)
    }
    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.set_selected_text_range(range_utf16, window, cx)
    }
    fn element_bounds(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<gpui::Pixels>> {
        self.0.element_bounds(window, cx)
    }
    fn text_length_utf16(&mut self, window: &mut Window, cx: &mut App) -> Option<usize> {
        self.0.text_length_utf16(window, cx)
    }
    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
    fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.0.accepts_text_input(window, cx)
    }
    fn text_input_editable_range(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        self.0.text_input_editable_range(window, cx)
    }
    /// Raw keys must reach the program: with a Japanese, Korean or Chinese
    /// source active, `true` would send printable keys (including Option-letter,
    /// the Meta key) to the input method before `key_down` sees them.
    fn prefers_ime_for_printable_keys(&mut self, _: &mut Window, _: &mut App) -> bool {
        false
    }
    fn text_input_configuration(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::TextInputConfiguration {
        self.0.text_input_configuration(window, cx)
    }
}

/// One shaped run at a column, or a filled cell span.
struct Painted {
    lines: Vec<(gpui::Point<gpui::Pixels>, ShapedLine)>,
    rects: Vec<(Bounds<gpui::Pixels>, Hsla)>,
    cursor: Option<(Bounds<gpui::Pixels>, Hsla, bool)>,
    line_height: gpui::Pixels,
    /// Set while the pointer is over a link, for the pointing hand.
    link_hitbox: Option<gpui::Hitbox>,
    /// The input method's composition, drawn underlined at the cursor.
    marked: Option<MarkedPaint>,
}

struct MarkedPaint {
    backdrop: Bounds<gpui::Pixels>,
    fill: Hsla,
    origin: gpui::Point<gpui::Pixels>,
    line: ShapedLine,
}

struct Grid {
    terminal: Entity<Terminal>,
    focused: bool,
}
impl IntoElement for Grid {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
/// A cell to paint: column, glyph, pen, fill and whether it is wide.
type Painting = (usize, char, Pen, Option<Hsla>, bool);
#[derive(Clone, PartialEq)]
struct Pen {
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}
impl Element for Grid {
    type RequestLayoutState = ();
    type PrepaintState = Painted;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = gpui::relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Painted {
        let font_size = px(FONT_SIZE);
        let base = crate::fonts::terminal(cx);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&base);
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map_or(px(7.8), |size| size.width);
        let line_height = (text_system.ascent(font_id, font_size)
            + text_system.descent(font_id, font_size).abs())
        .ceil()
        .max(px(14.));
        let origin = point(bounds.left() + px(PADDING), bounds.top() + px(PADDING / 2.));
        let columns =
            (((bounds.size.width - px(PADDING * 2.)) / cell_width).floor() as usize).max(2);
        let lines = (((bounds.size.height - px(PADDING)) / line_height).floor() as usize).max(1);
        let cell = size(cell_width, line_height);
        let (term, link, marked_text) = self.terminal.update(cx, |t, _| {
            t.resize(columns, lines, cell);
            t.geometry.set(Some(Geometry { origin, cell }));
            let link = t.hover.and_then(|position| t.link_at(position));
            let marked = t.marked.as_ref().map(|m| m.text.clone());
            (t.term.clone(), link, marked)
        });

        let term = term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset;
        let selection = content.selection;
        let default_fg = color(AnsiColor::Named(NamedColor::Foreground), colors, true);
        // The program's OSC 11 background, else the chat fill.
        let default_bg = color(AnsiColor::Named(NamedColor::Background), colors, true);
        let mut rows: Vec<Vec<Painting>> = vec![Vec::new(); lines];
        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let Some(at) = point_to_viewport(offset, indexed.point) else {
                continue;
            };
            if at.line >= lines {
                continue;
            }
            let mut fg = color(cell.fg, colors, true);
            let mut bg = match cell.bg {
                AnsiColor::Named(NamedColor::Background) => None,
                other => Some(color(other, colors, false)),
            };
            if cell.flags.contains(Flags::INVERSE) {
                let back = bg.unwrap_or(rgb(CHAT).into());
                bg = Some(fg);
                fg = back;
            }
            if cell.flags.intersects(Flags::DIM) {
                fg.a *= 0.66;
            }
            if selection.is_some_and(|s| s.contains(indexed.point)) {
                bg = Some(selection_fill());
            }
            let c = if cell.flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                cell.c
            };
            rows[at.line].push((
                at.column.0,
                c,
                Pen {
                    fg,
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES)
                        || link
                            .as_ref()
                            .is_some_and(|link| link.range.contains(&indexed.point)),
                    strike: cell.flags.contains(Flags::STRIKEOUT),
                },
                bg,
                cell.flags.contains(Flags::WIDE_CHAR),
            ));
        }
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;
        let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);
        drop(term);

        let mut painted = Painted {
            lines: Vec::new(),
            rects: Vec::new(),
            cursor: None,
            line_height,
            link_hitbox: link.map(|_| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal)),
            marked: None,
        };
        for (line, cells) in rows.iter().enumerate() {
            let y = origin.y + line_height * line as f32;
            // Backgrounds merge across equal neighbours.
            let mut span: Option<(usize, usize, Hsla)> = None;
            for (column, _, _, bg, wide) in cells {
                let width = if *wide { 2 } else { 1 };
                match (span, bg) {
                    (Some((start, end, fill)), Some(bg)) if end == *column && fill == *bg => {
                        span = Some((start, end + width, fill));
                    }
                    (_, bg) => {
                        if let Some((start, end, fill)) = span.take() {
                            painted
                                .rects
                                .push((cell_rect(origin, cell, line, start, end), fill));
                        }
                        span = bg.map(|bg| (*column, column + width, bg));
                    }
                }
            }
            if let Some((start, end, fill)) = span {
                painted
                    .rects
                    .push((cell_rect(origin, cell, line, start, end), fill));
            }
            // Text runs break where the pen changes, and around wide glyphs
            // so each one spans its two cells.
            let mut ix = 0;
            while ix < cells.len() {
                let (column, _, pen, _, wide) = &cells[ix];
                let mut text = String::new();
                let mut end = ix;
                while end < cells.len()
                    && cells[end].2 == *pen
                    && !cells[end].4
                    && (end == ix || !wide)
                    && cells[end].0 == column + (end - ix)
                {
                    text.push(cells[end].1);
                    end += 1;
                }
                if end == ix {
                    text.push(cells[ix].1);
                    end = ix + 1;
                }
                if !text.trim().is_empty() {
                    let mut font = base.clone();
                    if pen.bold {
                        font.weight = FontWeight::BOLD;
                    }
                    if pen.italic {
                        font.style = FontStyle::Italic;
                    }
                    let run = TextRun {
                        len: text.len(),
                        font,
                        color: pen.fg,
                        background_color: None,
                        underline: pen.underline.then_some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(pen.fg),
                            wavy: false,
                        }),
                        strikethrough: pen.strike.then_some(StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(pen.fg),
                        }),
                    };
                    let width = if *wide { cell_width * 2. } else { cell_width };
                    let shaped = text_system.shape_line(
                        SharedString::from(text),
                        font_size,
                        &[run],
                        Some(width),
                    );
                    painted
                        .lines
                        .push((point(origin.x + cell_width * *column as f32, y), shaped));
                }
                ix = end;
            }
        }
        if show_cursor
            && cursor_shape != CursorShape::Hidden
            && let Some(at) = point_to_viewport(offset, cursor_point)
            && at.line < lines
        {
            let x = origin.x + cell_width * at.column.0 as f32;
            let y = origin.y + line_height * at.line as f32;
            let accent: Hsla = accent_color();
            let (bounds, hollow) = match (cursor_shape, self.focused) {
                (CursorShape::Beam, _) => {
                    (Bounds::new(point(x, y), size(px(2.), line_height)), false)
                }
                (CursorShape::Underline, _) => (
                    Bounds::new(point(x, y + line_height - px(2.)), size(cell_width, px(2.))),
                    false,
                ),
                (CursorShape::Block, true) => (Bounds::new(point(x, y), cell), false),
                _ => (Bounds::new(point(x, y), cell), true),
            };
            let mut fill = accent;
            if !self.focused {
                fill.a = 0.5;
            } else if cursor_shape == CursorShape::Block {
                fill.a = 0.6;
            }
            painted.cursor = Some((bounds, fill, hollow));
        }
        if let Some(text) = marked_text.filter(|text| !text.is_empty())
            && let Some(at) = point_to_viewport(offset, cursor_point)
            && at.line < lines
        {
            let origin = point(
                origin.x + cell_width * at.column.0 as f32,
                origin.y + line_height * at.line as f32,
            );
            let run = TextRun {
                len: text.len(),
                font: base.clone(),
                color: default_fg,
                background_color: None,
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(default_fg),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let line = text_system.shape_line(SharedString::from(text), font_size, &[run], None);
            painted.marked = Some(MarkedPaint {
                backdrop: Bounds::new(origin, size(line.width.max(cell_width), line_height)),
                fill: default_bg,
                origin,
                line,
            });
        }
        painted
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        _: &mut (),
        painted: &mut Painted,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Element mouse handlers only receive hovered events. An active drag
        // must keep tracking movement and release anywhere in the window.
        let terminal = self.terminal.downgrade();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == gpui::DispatchPhase::Capture {
                let _ = terminal.update(cx, |terminal, cx| terminal.mouse_move(event, cx));
            }
        });
        let terminal = self.terminal.downgrade();
        window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
            if phase == gpui::DispatchPhase::Capture && event.button == MouseButton::Left {
                let _ = terminal.update(cx, |terminal, cx| terminal.mouse_up(event, cx));
            }
        });
        if let Some(hitbox) = &painted.link_hitbox {
            window.set_cursor_style(gpui::CursorStyle::PointingHand, hitbox);
        }
        let focus = self.terminal.read(cx).focus.clone();
        window.handle_input(
            &focus,
            TerminalInputHandler(ElementInputHandler::new(bounds, self.terminal.clone())),
            cx,
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (rect, fill) in &painted.rects {
                window.paint_quad(gpui::fill(*rect, *fill));
            }
            for (origin, line) in &painted.lines {
                let _ = line.paint(
                    *origin,
                    painted.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some((rect, fill, hollow)) = painted.cursor {
                if hollow {
                    window.paint_quad(gpui::outline(rect, fill, gpui::BorderStyle::Solid));
                } else {
                    window.paint_quad(gpui::fill(rect, fill));
                }
            }
            if let Some(marked) = &painted.marked {
                window.paint_quad(gpui::fill(marked.backdrop, marked.fill));
                let _ = marked.line.paint(
                    marked.origin,
                    painted.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        });
    }
}

fn cell_rect(
    origin: gpui::Point<gpui::Pixels>,
    cell: Size<gpui::Pixels>,
    line: usize,
    start: usize,
    end: usize,
) -> Bounds<gpui::Pixels> {
    Bounds::new(
        point(
            origin.x + cell.width * start as f32,
            origin.y + cell.height * line as f32,
        ),
        size(cell.width * (end - start) as f32, cell.height),
    )
}

/// The HTTP(S) link covering `at`: the OSC 8 hyperlink on its cells, else
/// a plain URL on its logical line.
fn link_at<T>(term: &Term<T>, regex: &mut RegexSearch, at: GridPoint) -> Option<Link> {
    let start = term.line_search_left(at);
    let end = term.line_search_right(at);
    if let Some(hyperlink) = term.grid()[at].hyperlink() {
        let same = |point: GridPoint| term.grid()[point].hyperlink().as_ref() == Some(&hyperlink);
        let mut first = at;
        while first > start && same(first.sub(term, Boundary::None, 1)) {
            first = first.sub(term, Boundary::None, 1);
        }
        let mut last = at;
        while last < end && same(last.add(term, Boundary::None, 1)) {
            last = last.add(term, Boundary::None, 1);
        }
        let uri = hyperlink.uri();
        return is_web_link(uri).then(|| Link {
            range: first..=last,
            uri: uri.to_owned(),
        });
    }
    RegexIter::new(start, end, Direction::Right, term, regex)
        .find(|found| found.contains(&at))
        .map(|range| {
            let mut uri = String::new();
            let mut point = *range.start();
            loop {
                let cell = &term.grid()[point];
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    uri.push(cell.c);
                }
                if point >= *range.end() {
                    break;
                }
                point = point.add(term, Boundary::None, 1);
            }
            Link { range, uri }
        })
        .filter(|link| is_web_link(&link.uri))
}
/// Only HTTP(S) opens, as TerminalView's `openTerminalLink` allows.
fn is_web_link(uri: &str) -> bool {
    let lower = uri.get(..8).unwrap_or(uri).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}
fn selection_fill() -> Hsla {
    crate::theme::text_selection()
}
fn write_note(term: &Arc<FairMutex<Term<Listener>>>, text: &str) {
    let mut parser = alacritty_terminal::vte::ansi::Processor::<
        alacritty_terminal::vte::ansi::StdSyncHandler,
    >::new();
    parser.advance(&mut *term.lock(), text.as_bytes());
}

/// TerminalView's ANSI_DARK palette.
const ANSI: [u32; 16] = [
    0x1d2428, 0xf87171, 0x4ade80, 0xfbbf24, 0x60a5fa, 0xc084fc, 0x22d3ee, 0xe8eef2, 0x64748b,
    0xfca5a5, 0x86efac, 0xfde68a, 0x93c5fd, 0xd8b4fe, 0x67e8f9, 0xf8fafc,
];
/// oscColors(): what a CLI asking for colours is told.
const OSC_FOREGROUND: u32 = TEXT;
const OSC_BACKGROUND: u32 = 0x171717;

fn indexed_rgb(index: u8) -> u32 {
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        _ => {
            let v = 8 + (index as u32 - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    }
}
fn hex(rgb: Rgb) -> u32 {
    ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
}
fn to_rgb(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}
/// Reply colour for an OSC 4/10/11/12 query: an app-set colour wins.
fn color_rgb(index: usize, colors: &Colors) -> Rgb {
    if let Some(rgb) = colors[index] {
        return rgb;
    }
    to_rgb(match index {
        0..=255 => indexed_rgb(index as u8),
        256 => OSC_FOREGROUND,
        257 => OSC_BACKGROUND,
        258 => ACCENT,
        _ => OSC_FOREGROUND,
    })
}
fn color(value: AnsiColor, colors: &Colors, foreground: bool) -> Hsla {
    let dim = |c: u32| {
        let mut h: Hsla = rgb(c).into();
        h.a *= 0.66;
        h
    };
    match value {
        AnsiColor::Spec(spec) => rgb(hex(spec)).into(),
        AnsiColor::Indexed(index) => colors[index as usize]
            .map_or_else(|| rgb(indexed_rgb(index)).into(), |c| rgb(hex(c)).into()),
        AnsiColor::Named(name) => {
            if let Some(c) = colors[name] {
                return rgb(hex(c)).into();
            }
            match name {
                NamedColor::Foreground | NamedColor::BrightForeground => content(1.),
                NamedColor::DimForeground => content(0.66),
                NamedColor::Background => {
                    if foreground {
                        rgb(CHAT).into()
                    } else {
                        gpui::transparent_black()
                    }
                }
                NamedColor::Cursor => accent_color(),
                NamedColor::DimBlack => dim(ANSI[0]),
                NamedColor::DimRed => dim(ANSI[1]),
                NamedColor::DimGreen => dim(ANSI[2]),
                NamedColor::DimYellow => dim(ANSI[3]),
                NamedColor::DimBlue => dim(ANSI[4]),
                NamedColor::DimMagenta => dim(ANSI[5]),
                NamedColor::DimCyan => dim(ANSI[6]),
                NamedColor::DimWhite => dim(ANSI[7]),
                other => rgb(ANSI[(other as usize).min(15)]).into(),
            }
        }
    }
}

/// Bytes a keystroke sends, or None to leave it to the app's bindings or, for
/// plain typed text, to the input handler (see `Terminal::key_down`).
/// Includes terminalKeys.ts's macOS editing shortcuts.
pub(crate) fn key_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    if m.platform {
        if m.control || m.shift || m.alt {
            return None;
        }
        return match key {
            "left" => Some(b"\x01".to_vec()),
            "right" => Some(b"\x05".to_vec()),
            "backspace" => Some(b"\x15".to_vec()),
            _ => None,
        };
    }
    if m.alt && !m.control && !m.shift {
        match key {
            "left" => return Some(b"\x1bb".to_vec()),
            "right" => return Some(b"\x1bf".to_vec()),
            _ => {}
        }
    }
    let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let cursor = |final_byte: char| {
        Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        })
    };
    let tilde = |code: u8| {
        Some(if modifier > 1 {
            format!("\x1b[{code};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        })
    };
    let function = |final_byte: char| {
        Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else {
            format!("\x1bO{final_byte}").into_bytes()
        })
    };
    let meta = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if m.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        Some(out)
    };
    match key {
        "up" => return cursor('A'),
        "down" => return cursor('B'),
        "right" => return cursor('C'),
        "left" => return cursor('D'),
        "home" => return cursor('H'),
        "end" => return cursor('F'),
        "insert" => return tilde(2),
        "delete" => return tilde(3),
        "pageup" => return tilde(5),
        "pagedown" => return tilde(6),
        "f1" => return function('P'),
        "f2" => return function('Q'),
        "f3" => return function('R'),
        "f4" => return function('S'),
        "f5" => return tilde(15),
        "f6" => return tilde(17),
        "f7" => return tilde(18),
        "f8" => return tilde(19),
        "f9" => return tilde(20),
        "f10" => return tilde(21),
        "f11" => return tilde(23),
        "f12" => return tilde(24),
        "enter" => return meta(b"\r"),
        "escape" => return meta(b"\x1b"),
        "tab" if m.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return meta(b"\t"),
        "backspace" if m.control => return meta(b"\x08"),
        "backspace" => return meta(b"\x7f"),
        _ => {}
    }
    if m.control {
        let ch = if key == "space" {
            ' '
        } else {
            let mut chars = key.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            ch
        };
        let code = match ch.to_ascii_lowercase() {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            ' ' | '@' | '2' => 0,
            '[' | '3' => 27,
            '\\' | '4' => 28,
            ']' | '5' => 29,
            '^' | '6' => 30,
            '_' | '-' | '/' | '7' => 31,
            '8' => 127,
            _ => return None,
        };
        return meta(&[code]);
    }
    // Option is Meta (`macOptionIsMeta`): the key itself, after ESC.
    if m.alt {
        let text = if key == "space" {
            " ".to_owned()
        } else if m.shift {
            key.to_uppercase()
        } else {
            key.to_owned()
        };
        if text.chars().count() == 1 {
            return meta(text.as_bytes());
        }
    }
    None
}

/// Signal the program and its process group; escalate off the calling thread.
#[cfg(unix)]
fn terminate(pid: u32) {
    if pid <= 1 {
        return;
    }
    let pid = pid as i32;
    let signal = move |sig: i32| unsafe {
        libc::kill(pid, sig);
        libc::kill(-pid, sig);
    };
    let alive = move || unsafe { libc::kill(pid, 0) == 0 || libc::kill(-pid, 0) == 0 };
    signal(libc::SIGHUP);
    signal(libc::SIGTERM);
    std::thread::spawn(move || {
        let deadline = Instant::now() + KILL_ESCALATE;
        while Instant::now() < deadline {
            if !alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if alive() {
            signal(libc::SIGKILL);
        }
    });
}
#[cfg(not(unix))]
fn terminate(_: u32) {}

/// The last `TAIL_CHARS` characters.
fn tail(text: &str) -> String {
    let skip = text.chars().count().saturating_sub(TAIL_CHARS);
    text.chars().skip(skip).collect()
}
/// Everything in the grid, scrollback included, as the user read it: no styling,
/// no padding, rows joined unless the line wrapped, trailing blank rows dropped.
fn grid_text(term: &Term<Listener>) -> String {
    let grid = term.grid();
    let mut out = String::new();
    for line in grid.topmost_line().0..=grid.bottommost_line().0 {
        let row = &grid[alacritty_terminal::index::Line(line)];
        let mut text = String::new();
        for column in 0..grid.columns() {
            let cell = &row[Column(column)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            text.push(cell.c);
            for &mark in cell.zerowidth().unwrap_or_default() {
                text.push(mark);
            }
        }
        let wrapped = row[Column(grid.columns() - 1)]
            .flags
            .contains(Flags::WRAPLINE);
        if wrapped {
            out.push_str(&text);
        } else {
            out.push_str(text.trim_end());
            out.push('\n');
        }
    }
    out.trim_end().to_owned()
}

fn default_title(cwd: &str) -> String {
    let name = std::path::Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.is_empty() || name == "/" {
        "Terminal".into()
    } else {
        name
    }
}
fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
/// The home directory as a terminal cwd, or `~` (which `working_dir` expands)
/// when the platform reports none.
pub(crate) fn home_cwd() -> String {
    home().map_or_else(|| "~".into(), |h| h.to_string_lossy().into_owned())
}
fn working_dir(cwd: &str) -> std::path::PathBuf {
    let path = match cwd.strip_prefix("~") {
        Some(rest) => home().map_or_else(|| cwd.into(), |h| h.join(rest.trim_start_matches('/'))),
        None => std::path::PathBuf::from(cwd),
    };
    if path.is_dir() {
        path
    } else {
        home().unwrap_or(path)
    }
}
fn default_shell() -> (String, Vec<String>) {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/bash".into()
            }
        });
    let stem = std::path::Path::new(&shell)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let args = match stem.as_str() {
        "zsh" | "bash" | "sh" | "fish" => vec!["-l".to_owned()],
        _ => Vec::new(),
    };
    (shell, args)
}

/// pty.rs's foreground_label: the command holding the terminal, unless it
/// is the shell itself.
#[cfg(unix)]
fn foreground_label(master_fd: i32, shell_pid: u32) -> Option<String> {
    if master_fd < 0 {
        return None;
    }
    let pgrp = unsafe { libc::tcgetpgrp(master_fd) };
    if pgrp <= 0 || pgrp == shell_pid as i32 {
        return None;
    }
    let output = std::process::Command::new("ps")
        .args(["-p", &pgrp.to_string(), "-o", "args="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let label = command_label(String::from_utf8_lossy(&output.stdout).trim())?;
    (!is_shell_name(&label)).then_some(label)
}
#[cfg(not(unix))]
fn foreground_label(_: i32, _: u32) -> Option<String> {
    None
}
/// Interpreters name the script they run: `node vite.js` reads `vite.js`.
pub(crate) fn command_label(args: &str) -> Option<String> {
    let mut parts = args.split_whitespace();
    let exe = parts.next()?;
    let base = std::path::Path::new(exe).file_name()?.to_str()?.to_owned();
    let base = base.trim_start_matches('-').to_owned();
    if matches!(
        base.as_str(),
        "node" | "nodejs" | "python" | "python3" | "ruby" | "deno" | "bun"
    ) {
        for part in parts {
            if part.starts_with('-') {
                continue;
            }
            let name = std::path::Path::new(part).file_name()?.to_str()?;
            if !name.starts_with('-') {
                return Some(name.to_owned());
            }
        }
    }
    Some(base)
}
/// The supported agent CLI a foreground label names, as a provider id, so
/// its tab can show the provider's mark instead of the terminal icon.
pub(crate) fn agent_cli(label: &str) -> Option<&'static str> {
    // Script launchers report the script: `codex.js` is Codex.
    let name = label.split('.').next()?;
    match name {
        "cursor-agent" | "agent" => return Some("cursor"),
        "pi-coding-agent" => return Some("pi"),
        _ => {}
    }
    monocode_core::provider::Kind::parse(name).map(|kind| kind.id())
}
impl Terminal {
    /// The agent CLI holding this terminal, if any.
    pub(crate) fn agent(&self) -> Option<&'static str> {
        agent_cli(self.foreground.as_deref()?)
    }
}
fn is_shell_name(name: &str) -> bool {
    matches!(
        name,
        "zsh" | "bash" | "sh" | "fish" | "nu" | "dash" | "ksh" | "tcsh" | "zsh5"
    )
}

/// One wheel notch as a mouse report, SGR when asked for, else the legacy
/// X10 encoding (which cannot address past column or line 223).
fn wheel_report(up: bool, line: usize, column: usize, mode: TermMode) -> Vec<u8> {
    let button = if up { 64 } else { 65 };
    if mode.contains(TermMode::SGR_MOUSE) {
        format!("\x1b[<{button};{};{}M", column + 1, line + 1).into_bytes()
    } else {
        let cell = |n: usize| (n.min(222) + 33) as u8;
        vec![0x1b, b'[', b'M', button + 32, cell(column), cell(line)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::Line;
    use gpui::{AppContext, Modifiers};

    #[test]
    fn wheel_reports_match_the_requested_encoding() {
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(wheel_report(true, 4, 9, sgr), b"\x1b[<64;10;5M");
        assert_eq!(wheel_report(false, 0, 0, sgr), b"\x1b[<65;1;1M");
        assert_eq!(
            wheel_report(true, 4, 9, TermMode::MOUSE_REPORT_CLICK),
            [0x1b, b'[', b'M', 96, 42, 37]
        );
    }

    fn key(spec: &str) -> Keystroke {
        Keystroke::parse(spec).unwrap()
    }
    fn bytes(spec: &str) -> Option<Vec<u8>> {
        key_bytes(&key(spec), TermMode::empty())
    }

    #[gpui::test]
    fn links_cover_plain_urls_and_osc8_hyperlinks(cx: &mut gpui::TestAppContext) {
        let view = cx.new(|cx| Terminal::new("/tmp".into(), cx));
        view.update(cx, |terminal, _| {
            let cell = size(px(8.), px(16.));
            terminal.resize(20, 4, cell);
            terminal.geometry.set(Some(Geometry {
                origin: point(px(0.), px(0.)),
                cell,
            }));
            // A URL wrapping onto the next row, trailing punctuation
            // excluded; then an OSC 8 link and a file:// one.
            write_note(
                &terminal.term,
                "see https://example.com/a/b. ok\r\n\x1b]8;;https://x.dev\x1b\\docs\x1b]8;;\x1b\\ \
                 \x1b]8;;file:///tmp\x1b\\tmp\x1b]8;;\x1b\\ ftp://a.b",
            );
            let at = |line: usize, column: usize| {
                point(px(column as f32 * 8. + 1.), px(line as f32 * 16. + 1.))
            };
            let url = terminal.link_at(at(1, 2)).expect("wrapped URL");
            assert_eq!(url.uri, "https://example.com/a/b");
            assert_eq!(
                url.range,
                GridPoint::new(Line(0), Column(4))..=GridPoint::new(Line(1), Column(6))
            );
            assert_eq!(terminal.link_at(at(0, 4)), Some(url));
            assert_eq!(terminal.link_at(at(0, 1)), None);
            assert_eq!(terminal.link_at(at(1, 7)), None);

            let docs = terminal.link_at(at(2, 1)).expect("OSC 8 link");
            assert_eq!(docs.uri, "https://x.dev");
            assert_eq!(
                docs.range,
                GridPoint::new(Line(2), Column(0))..=GridPoint::new(Line(2), Column(3))
            );
            assert_eq!(terminal.link_at(at(2, 6)), None, "file:// stays inert");
            assert_eq!(terminal.link_at(at(2, 10)), None, "ftp:// stays inert");
            assert_eq!(terminal.link_at(point(px(-4.), px(1.))), None);
        });
    }

    #[gpui::test]
    fn dragging_at_edges_extends_selection_through_history(cx: &mut gpui::TestAppContext) {
        let view = cx.new(|cx| Terminal::new("/tmp".into(), cx));
        view.update(cx, |terminal, cx| {
            let cell = size(px(8.), px(16.));
            terminal.resize(40, 4, cell);
            terminal.geometry.set(Some(Geometry {
                origin: point(px(0.), px(0.)),
                cell,
            }));
            let output = (0..30)
                .map(|n| format!("line-{n:02}"))
                .collect::<Vec<_>>()
                .join("\r\n");
            write_note(&terminal.term, &output);
            terminal.term.lock().selection = Some(Selection::new(
                SelectionType::Simple,
                GridPoint::new(Line(3), Column(6)),
                Side::Right,
            ));
            terminal.selecting = true;
            // Holding a click alone must not scroll.
            assert!(terminal.scroll_selection(cx));
            assert_eq!(terminal.term.lock().grid().display_offset(), 0);
            terminal.selection_pointer = Some(point(px(0.), px(0.)));
            assert!(terminal.scroll_selection(cx));
            assert_eq!(terminal.term.lock().grid().display_offset(), 1);
            let text = terminal.term.lock().selection_to_string().unwrap();
            assert!(text.contains("line-25"), "{text}");
            assert!(text.contains("line-29"), "{text}");
            // A second tick at the same pointer position must keep extending.
            assert!(terminal.scroll_selection(cx));
            assert!(
                terminal
                    .term
                    .lock()
                    .selection_to_string()
                    .unwrap()
                    .contains("line-24")
            );
            terminal.selection_pointer = Some(point(px(0.), px(32.)));
            terminal.scroll_selection(cx);
            assert_eq!(terminal.term.lock().grid().display_offset(), 2);
            // Dragging past the bottom returns toward the live screen.
            terminal.selection_pointer = Some(point(px(0.), px(80.)));
            terminal.scroll_selection(cx);
            assert_eq!(terminal.term.lock().grid().display_offset(), 0);
            terminal.stop_selection();
            assert!(!terminal.scroll_selection(cx));
        });
    }

    #[test]
    fn keys_encode_like_xterm_with_the_macos_editing_shortcuts() {
        assert_eq!(bytes("enter"), Some(b"\r".to_vec()));
        assert_eq!(bytes("backspace"), Some(b"\x7f".to_vec()));
        assert_eq!(bytes("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            key_bytes(&key("up"), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(bytes("shift-right"), Some(b"\x1b[1;2C".to_vec()));
        assert_eq!(bytes("ctrl-c"), Some(vec![3]));
        assert_eq!(bytes("ctrl-["), Some(vec![27]));
        assert_eq!(bytes("pagedown"), Some(b"\x1b[6~".to_vec()));
        assert_eq!(bytes("f5"), Some(b"\x1b[15~".to_vec()));
        assert_eq!(bytes("shift-tab"), Some(b"\x1b[Z".to_vec()));
        // terminalKeys.ts
        assert_eq!(bytes("alt-left"), Some(b"\x1bb".to_vec()));
        assert_eq!(bytes("alt-right"), Some(b"\x1bf".to_vec()));
        assert_eq!(bytes("cmd-left"), Some(b"\x01".to_vec()));
        assert_eq!(bytes("cmd-right"), Some(b"\x05".to_vec()));
        assert_eq!(bytes("cmd-backspace"), Some(b"\x15".to_vec()));
        // Option is Meta.
        assert_eq!(bytes("alt-b"), Some(b"\x1bb".to_vec()));
        // App shortcuts stay with the app.
        assert_eq!(bytes("cmd-j"), None);
        assert_eq!(bytes("cmd-shift-d"), None);
    }

    fn typed(key: &str, key_char: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: Some(key_char.into()),
        }
    }

    #[test]
    fn typed_text_is_left_to_the_input_handler() {
        let none = Modifiers::default();
        let shift = Modifiers::shift();
        for keystroke in [
            typed("a", "a", none),
            typed("a", "A", shift),
            typed("space", " ", none),
            typed("1", "!", shift),
            // An input method rewrote the text: the key code says A, the
            // characters say otherwise.
            typed("a", "ô", none),
            typed("a", "đ", none),
            typed("a", "ếng", none),
        ] {
            assert_eq!(
                key_bytes(&keystroke, TermMode::empty()),
                None,
                "{keystroke:?}"
            );
        }
        // Keys that are not text keep their bytes, whatever key_char says.
        assert_eq!(
            key_bytes(&typed("enter", "\n", none), TermMode::empty()),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            key_bytes(&typed("tab", "\t", none), TermMode::empty()),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            key_bytes(&typed("b", "\u{222b}", Modifiers::alt()), TermMode::empty()),
            Some(b"\x1bb".to_vec())
        );
    }

    fn terminal_window(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<Terminal>, &mut gpui::VisualTestContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| Terminal::new("/tmp".into(), cx));
        terminal.update_in(cx, |terminal, window, cx| window.focus(&terminal.focus, cx));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        (terminal, cx)
    }
    fn sent(terminal: &Entity<Terminal>, cx: &gpui::VisualTestContext) -> Vec<u8> {
        terminal.read_with(cx, |terminal, _| terminal.sent.borrow().clone())
    }
    fn press(
        terminal: &Entity<Terminal>,
        cx: &mut gpui::VisualTestContext,
        keystroke: Keystroke,
    ) -> Vec<u8> {
        terminal.read_with(cx, |terminal, _| terminal.sent.borrow_mut().clear());
        terminal.update(cx, |terminal, cx| {
            terminal.key_down(
                &KeyDownEvent {
                    keystroke,
                    is_held: false,
                    prefer_character_input: false,
                },
                cx,
            )
        });
        sent(terminal, cx)
    }

    #[gpui::test]
    fn key_down_writes_special_keys_but_not_typed_text(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = terminal_window(cx);
        let none = Modifiers::default();
        // Typed text reaches the PTY only through the input handler.
        for keystroke in [
            typed("a", "a", none),
            typed("space", " ", none),
            typed("a", "ô", none),
        ] {
            assert_eq!(press(&terminal, cx, keystroke), b"", "typed text");
        }
        // Everything else writes what it always did.
        let ctrl = Modifiers::control();
        let alt = Modifiers::alt();
        let cases: [(Keystroke, &[u8]); 7] = [
            (typed("c", "c", ctrl), b"\x03"),
            (typed("b", "\u{222b}", alt), b"\x1bb"),
            (key("up"), b"\x1b[A"),
            (typed("enter", "\n", none), b"\r"),
            (key("backspace"), b"\x7f"),
            (typed("tab", "\t", none), b"\t"),
            (key("alt-left"), b"\x1bb"),
        ];
        for (keystroke, expected) in cases {
            assert_eq!(
                press(&terminal, cx, keystroke.clone()),
                expected,
                "{keystroke:?}"
            );
        }
    }

    #[gpui::test]
    fn committed_text_is_written_as_utf8_once(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = terminal_window(cx);
        for text in ["ô", "đ", "ếng", "日本語"] {
            terminal.update_in(cx, |terminal, window, cx| {
                terminal.sent.borrow_mut().clear();
                terminal.replace_text_in_range(None, text, window, cx);
            });
            assert_eq!(sent(&terminal, cx), text.as_bytes(), "{text}");
        }
    }

    #[gpui::test]
    fn marked_text_stays_out_of_the_pty_until_committed(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = terminal_window(cx);
        terminal.update_in(cx, |terminal, window, cx| {
            terminal.replace_and_mark_text_in_range(None, "ê", Some(1..1), window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), Some(0..1));
            terminal.replace_and_mark_text_in_range(None, "ếng", None, window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), Some(0..3));
            let mut adjusted = None;
            assert_eq!(
                terminal
                    .text_for_range(1..3, &mut adjusted, window, cx)
                    .as_deref(),
                Some("ng")
            );
            assert_eq!(adjusted, Some(1..3));
            assert_eq!(
                terminal
                    .selected_text_range(false, window, cx)
                    .map(|selection| selection.range),
                Some(3..3)
            );
        });
        assert_eq!(sent(&terminal, cx), b"", "composing writes nothing");

        terminal.update_in(cx, |terminal, window, cx| {
            terminal.replace_text_in_range(None, "ếng", window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), None);
        });
        assert_eq!(sent(&terminal, cx), "ếng".as_bytes(), "committed once");

        // A cancelled composition (empty marked text) writes nothing.
        terminal.update_in(cx, |terminal, window, cx| {
            terminal.sent.borrow_mut().clear();
            terminal.replace_and_mark_text_in_range(None, "x", None, window, cx);
            terminal.replace_and_mark_text_in_range(None, "", None, window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), None);
            terminal.unmark_text(window, cx);
        });
        assert_eq!(sent(&terminal, cx), b"");
    }

    #[gpui::test]
    fn unmarking_commits_the_composition_once(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = terminal_window(cx);
        terminal.update_in(cx, |terminal, window, cx| {
            terminal.replace_and_mark_text_in_range(None, "ô", None, window, cx);
            terminal.unmark_text(window, cx);
            terminal.unmark_text(window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), None);
        });
        assert_eq!(sent(&terminal, cx), "ô".as_bytes());
    }

    #[gpui::test]
    fn the_candidate_window_opens_at_the_cursor(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = terminal_window(cx);
        let at = |terminal: &Entity<Terminal>, cx: &mut gpui::VisualTestContext| {
            terminal.update_in(cx, |terminal, window, cx| {
                terminal.bounds_for_range(0..0, Bounds::default(), window, cx)
            })
        };
        let first = at(&terminal, cx).expect("cursor bounds after the first paint");
        terminal.update(cx, |terminal, cx| terminal.fake_output("ab", cx));
        let moved = at(&terminal, cx).unwrap();
        assert_eq!(moved.size, first.size);
        assert_eq!(moved.origin.y, first.origin.y);
        assert_eq!(moved.origin.x, first.origin.x + first.size.width * 2.);
    }

    #[gpui::test]
    fn typing_through_the_window_reaches_the_pty_exactly_once(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = terminal_window(cx);
        cx.simulate_input("abc");
        assert_eq!(sent(&terminal, cx), b"abc");

        // The input method posts the replacement as a key whose code is A and
        // whose characters are the real text.
        terminal.read_with(cx, |terminal, _| terminal.sent.borrow_mut().clear());
        cx.update(|window, cx| {
            window.dispatch_keystroke(typed("a", "ô", Modifiers::default()), cx)
        });
        assert_eq!(sent(&terminal, cx), "ô".as_bytes());

        // Space and enter are keys with a key_char: each is written once.
        terminal.read_with(cx, |terminal, _| terminal.sent.borrow_mut().clear());
        cx.simulate_keystrokes("space enter backspace tab ctrl-c");
        assert_eq!(sent(&terminal, cx), b" \r\x7f\t\x03");

        // macOS with GoTiengViet: the key event says "a" (from the key code)
        // and the real text "ô" arrives afterwards as insertText. The key
        // event must write nothing, or the output would be "aô".
        terminal.read_with(cx, |terminal, _| terminal.sent.borrow_mut().clear());
        assert_eq!(
            press(&terminal, cx, typed("a", "a", Modifiers::default())),
            b"",
            "key_down leaves typed text to the input handler"
        );
        terminal.update_in(cx, |terminal, window, cx| {
            terminal.replace_text_in_range(None, "ô", window, cx)
        });
        assert_eq!(sent(&terminal, cx), "ô".as_bytes());
    }

    #[gpui::test]
    fn the_input_handler_keeps_raw_keys_for_the_program(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = terminal_window(cx);
        let mut handler =
            TerminalInputHandler(ElementInputHandler::new(Bounds::default(), terminal));
        cx.update(|window, cx| {
            // Option-letter is Meta: an input method must not see it first.
            assert!(!handler.prefers_ime_for_printable_keys(window, cx));
            // Held keys repeat instead of opening the accent menu.
            assert!(!handler.apple_press_and_hold_enabled());
            assert!(handler.accepts_text_input(window, cx));
        });
    }

    #[test]
    fn agent_clis_map_to_their_provider() {
        assert_eq!(agent_cli("claude"), Some("claude"));
        assert_eq!(agent_cli("codex.js"), Some("codex"));
        assert_eq!(agent_cli("cursor-agent"), Some("cursor"));
        assert_eq!(agent_cli("pi-coding-agent"), Some("pi"));
        assert_eq!(agent_cli("vim"), None);
    }
    #[test]
    fn foreground_labels_name_the_script_an_interpreter_runs() {
        assert_eq!(
            command_label("node /x/node_modules/.bin/vite --port 3000").as_deref(),
            Some("vite")
        );
        assert_eq!(
            command_label("/usr/bin/python3 -m http.server").as_deref(),
            Some("http.server")
        );
        assert_eq!(
            command_label("cargo watch -x test").as_deref(),
            Some("cargo")
        );
        assert_eq!(command_label("-zsh").as_deref(), Some("zsh"));
        assert!(is_shell_name("zsh"));
        assert_eq!(default_title("/work/monocode"), "monocode");
        assert_eq!(default_title("/"), "Terminal");
    }

    #[test]
    fn colours_follow_the_dark_palette_and_xterm_cube() {
        assert_eq!(indexed_rgb(1), 0xf87171);
        assert_eq!(indexed_rgb(16), 0x000000);
        assert_eq!(indexed_rgb(196), 0xff0000);
        assert_eq!(indexed_rgb(232), 0x080808);
        assert_eq!(indexed_rgb(255), 0xeeeeee);
        let colors = Colors::default();
        assert_eq!(hex(color_rgb(257, &colors)), OSC_BACKGROUND);
        assert_eq!(hex(color_rgb(258, &colors)), ACCENT);
    }

    fn collect(events: &Rc<std::cell::RefCell<Vec<TerminalEvent>>>) -> Vec<TerminalEvent> {
        events.borrow().clone()
    }
    fn launched_terminal(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<Terminal>, Rc<std::cell::RefCell<Vec<TerminalEvent>>>) {
        let events = Rc::new(std::cell::RefCell::new(Vec::new()));
        let terminal = cx.new(|cx| {
            Terminal::launch(
                Path::new("/tmp"),
                "/bin/claude".into(),
                vec!["--resume".into(), "abc".into()],
                vec![("CLAUDE_CONFIG_DIR".into(), "/acct".into())],
                vec!["ANTHROPIC_API_KEY"],
                cx,
            )
        });
        let sink = events.clone();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, event: &TerminalEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach()
        });
        (terminal, events)
    }
    #[gpui::test]
    fn a_launch_reports_start_title_and_exit_with_the_screen_tail(cx: &mut gpui::TestAppContext) {
        let (terminal, events) = launched_terminal(cx);
        terminal.update(cx, |t, cx| {
            let spec = t.launched().expect("the launch is recorded");
            assert_eq!(spec.program, OsString::from("/bin/claude"));
            assert_eq!(spec.args, ["--resume", "abc"].map(OsString::from));
            assert_eq!(spec.env_remove, ["ANTHROPIC_API_KEY"]);
            assert_eq!(
                spec.env_set,
                [("CLAUDE_CONFIG_DIR".to_owned(), OsString::from("/acct"))]
            );
            t.fake_started(cx);
            t.fake_title("  Fix the bug", cx);
            t.fake_title("  Fix the bug", cx);
            t.fake_output("\x1b[31mno conversation\x1b[0m found\r\n", cx);
            t.fake_exit(Some(3), cx);
            assert!(t.exited);
            assert_eq!(t.text(), "no conversation found\n\n[process exited (3)]");
            // A late exit changes nothing.
            t.fake_exit(Some(9), cx);
        });
        let seen = collect(&events);
        assert!(matches!(seen[0], TerminalEvent::Started { .. }), "{seen:?}");
        assert_eq!(seen[1], TerminalEvent::Title("  Fix the bug".into()));
        let exits: Vec<_> = seen
            .iter()
            .filter(|e| matches!(e, TerminalEvent::Exited { .. }))
            .collect();
        assert_eq!(
            exits,
            [&TerminalEvent::Exited {
                code: Some(3),
                error: None,
                tail: "no conversation found".into()
            }]
        );
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, TerminalEvent::Title(_)))
                .count(),
            1,
            "a repeated title is not forwarded twice"
        );
    }
    #[gpui::test]
    fn a_spawn_error_is_reported_as_an_exit_without_a_start(cx: &mut gpui::TestAppContext) {
        let (terminal, events) = launched_terminal(cx);
        terminal.update(cx, |t, cx| t.fake_spawn_error("No such file", cx));
        let seen = collect(&events);
        assert!(
            !seen
                .iter()
                .any(|e| matches!(e, TerminalEvent::Started { .. }))
        );
        assert!(seen.contains(&TerminalEvent::Exited {
            code: None,
            error: Some("No such file".into()),
            tail: "Could not start /bin/claude: No such file".into(),
        }));
        terminal.update(cx, |t, _| {
            assert!(t.exited);
            // Nothing to kill, and no panic.
            t.kill();
        });
    }
    #[gpui::test]
    fn the_dock_terminal_forwards_no_title_and_no_exit_details(cx: &mut gpui::TestAppContext) {
        let events = Rc::new(std::cell::RefCell::new(Vec::new()));
        let terminal = cx.new(|cx| Terminal::new("/tmp".into(), cx));
        let sink = events.clone();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, event: &TerminalEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach()
        });
        terminal.update(cx, |t, cx| {
            t.on_event(TermEvent::Title("vim".into()), cx);
            t.on_event(TermEvent::Exit, cx);
        });
        assert_eq!(collect(&events), [TerminalEvent::Changed]);
        assert!(terminal.read_with(cx, |t, _| t.exited));
    }

    #[cfg(unix)]
    #[test]
    fn env_removal_wraps_the_program_in_env_dash_u() {
        let (program, args) = wrap_removals(
            "/bin/claude".into(),
            vec!["--resume".into(), "abc".into()],
            &["ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        );
        assert_eq!(program, OsString::from("/usr/bin/env"));
        assert_eq!(
            args,
            [
                "-u",
                "ANTHROPIC_API_KEY",
                "-u",
                "OPENAI_API_KEY",
                "--",
                "/bin/claude",
                "--resume",
                "abc"
            ]
            .map(OsString::from)
        );
        // Nothing to remove, nothing wrapped.
        let (program, args) = wrap_removals("/bin/claude".into(), vec!["x".into()], &[]);
        assert_eq!(program, OsString::from("/bin/claude"));
        assert_eq!(args, [OsString::from("x")]);
    }
    #[test]
    fn the_exit_tail_is_the_last_4000_characters() {
        assert_eq!(tail("short"), "short");
        let long = format!("{}{}", "a".repeat(100), "b".repeat(TAIL_CHARS));
        assert_eq!(tail(&long), "b".repeat(TAIL_CHARS));
        // Counted in characters, never cutting inside one.
        let wide = "é".repeat(TAIL_CHARS + 5);
        assert_eq!(tail(&wide).chars().count(), TAIL_CHARS);
    }
    #[test]
    fn grid_text_is_what_the_user_saw_without_styling_or_padding() {
        let size = GridSize {
            columns: 10,
            lines: 4,
        };
        let spawned = detached(size);
        write_note(
            &spawned.term,
            "\x1b[31mred\x1b[0m line\r\nsecond   \r\n\r\n0123456789ABCDE\r\n",
        );
        // History is part of the text: the row scrolled off the top stays.
        write_note(&spawned.term, "tail\r\nmore\r\n");
        let text = grid_text(&spawned.term.lock());
        assert_eq!(text, "red line\nsecond\n\n0123456789ABCDE\ntail\nmore");
    }

    /// Runs a real program on a PTY; a plain test because GPUI's test scheduler
    /// refuses wakeups from the PTY thread.
    #[cfg(unix)]
    fn run_program(
        program: &str,
        args: &[&str],
        remove: &[&'static str],
    ) -> (String, Vec<TermEvent>) {
        let plan = Plan::launch(
            program.into(),
            args.iter().map(OsString::from).collect(),
            Vec::new(),
            remove,
        );
        let size = GridSize {
            columns: 60,
            lines: 10,
        };
        let spawned = spawn("/tmp", size, 2, plan);
        assert!(spawned.error.is_none(), "{:?}", spawned.error);
        assert!(spawned.shell_pid > 1);
        let mut events = Vec::new();
        for _ in 0..500 {
            match spawned.events.try_recv() {
                Ok(event) => {
                    let done = matches!(event, TermEvent::Exit);
                    events.push(event);
                    if done {
                        break;
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        (grid_text(&spawned.term.lock()), events)
    }
    #[cfg(unix)]
    #[test]
    fn a_launched_program_reports_its_exit_code_and_last_output() {
        let (text, events) = run_program("/bin/sh", &["-c", "echo goodbye; exit 7"], &[]);
        let code = events.iter().find_map(|e| match e {
            TermEvent::ChildExit(status) => Some(status.code()),
            _ => None,
        });
        assert_eq!(code, Some(Some(7)), "{events:?}");
        // The final output is drained before Exit, so the tail has it.
        assert!(matches!(events.last(), Some(TermEvent::Exit)));
        assert_eq!(text, "goodbye");
    }
    #[cfg(unix)]
    #[test]
    fn removed_env_vars_are_gone_in_the_launched_program() {
        // HOME is always set by the PTY, so its removal is observable.
        let (text, _) = run_program("/bin/sh", &["-c", "echo \"[${HOME-unset}]\""], &["HOME"]);
        assert_eq!(text, "[unset]");
        let (text, _) = run_program("/bin/sh", &["-c", "echo \"[${HOME-unset}]\""], &[]);
        assert_ne!(text, "[unset]");
    }
    #[cfg(unix)]
    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let plan = Plan::launch(
            "/nonexistent/monoptah-cli".into(),
            Vec::new(),
            Vec::new(),
            &[],
        );
        let size = GridSize {
            columns: 40,
            lines: 5,
        };
        let spawned = spawn("/tmp", size, 3, plan);
        assert!(spawned.sender.is_none());
        let error = spawned.error.expect("spawn failed");
        assert!(error.contains("No such file"), "{error}");
        assert!(
            grid_text(&spawned.term.lock())
                .starts_with("Could not start /nonexistent/monoptah-cli")
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_missing_program_is_a_spawn_error_even_under_the_env_wrapper() {
        for program in ["/nonexistent/monoptah-cli", "monoptah-no-such-cli"] {
            let plan = Plan::launch(
                program.into(),
                vec!["--resume".into()],
                Vec::new(),
                &["ANTHROPIC_API_KEY"],
            );
            // The wrapper would start fine and exit 127.
            assert_eq!(plan.program, "/usr/bin/env");
            let size = GridSize {
                columns: 80,
                lines: 5,
            };
            let spawned = spawn("/tmp", size, 4, plan);
            assert!(spawned.sender.is_none() && spawned.shell_pid == 0);
            let error = spawned.error.expect("reported as a spawn error");
            assert!(error.contains("No such file"), "{error}");
            let text = grid_text(&spawned.term.lock());
            assert!(
                text.starts_with(&format!("Could not start {program}:")),
                "{text}"
            );
            assert!(!text.contains("/usr/bin/env"), "{text}");
        }
    }
    #[cfg(unix)]
    #[test]
    fn program_lookup_follows_execvp() {
        let tmp = std::env::temp_dir().join(format!("monoptah-lookup-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let plain = tmp.join("plain");
        std::fs::write(&plain, "x").unwrap();
        let path = "/nonexistent:/bin";
        assert!(program_problem("sh", path, &tmp).is_none());
        assert!(program_problem("/bin/sh", "", &tmp).is_none());
        let missing = program_problem("nope-cli", path, &tmp).unwrap();
        assert_eq!(missing.raw_os_error(), Some(libc::ENOENT));
        // A directory is not a program.
        let dir = program_problem("/bin", "", &tmp).unwrap();
        assert_eq!(dir.raw_os_error(), Some(libc::ENOENT));
        let denied = program_problem(plain.to_str().unwrap(), "", &tmp).unwrap();
        assert_eq!(denied.raw_os_error(), Some(libc::EACCES));
        // A relative path is taken from the working directory.
        assert_eq!(
            program_problem("./plain", "/bin", &tmp)
                .unwrap()
                .raw_os_error(),
            Some(libc::EACCES)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
    #[cfg(unix)]
    #[test]
    fn kill_escalates_to_sigkill_when_hangup_and_terminate_are_ignored() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "trap '' HUP TERM; sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let started = std::time::Instant::now();
        terminate(child.id());
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "still alive");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert!(started.elapsed() >= Duration::from_millis(900));
    }

    #[test]
    fn a_login_shell_runs_and_its_output_reaches_the_grid() {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let size = GridSize {
            columns: 60,
            lines: 10,
        };
        let spawned = spawn(&dir, size, 1, Plan::shell());
        let sender = spawned.sender.expect("the shell started");
        assert!(spawned.shell_pid > 1);
        let _ = sender.send(Msg::Input(Cow::Borrowed(b"printf 'mono%scode\\n' -\r")));
        let screen = || {
            let term = spawned.term.lock();
            let grid = term.grid();
            (0..grid.screen_lines())
                .map(|line| {
                    (0..grid.columns())
                        .map(|c| grid[Line(line as i32)][Column(c)].c)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let mut seen = false;
        for _ in 0..250 {
            std::thread::sleep(Duration::from_millis(20));
            if screen().contains("mono-code") {
                seen = true;
                break;
            }
        }
        assert!(seen, "{}", screen());
        assert_eq!(spawned.term.lock().columns(), 60);
        // The shell owns the foreground while it waits at a prompt.
        assert_eq!(foreground_label(spawned.master_fd, spawned.shell_pid), None);
        let _ = sender.send(Msg::Input(Cow::Borrowed(b"exit\r")));
        let mut exited = false;
        for _ in 0..250 {
            std::thread::sleep(Duration::from_millis(20));
            if let Ok(TermEvent::ChildExit(_)) = spawned.events.try_recv() {
                exited = true;
                break;
            }
        }
        assert!(exited);
    }
}
