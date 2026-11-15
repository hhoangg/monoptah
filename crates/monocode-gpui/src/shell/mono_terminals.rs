//! Terminals a Mono opens and drives through `terminals.*` (see docs/agent-access.md).
//!
//! Terminals stay ordinary dock tabs or terminal panes. This module only adds a registry of
//! opaque ids (`t-<launch>-<n>`, the first part random per launch so a receipt from an earlier run cannot name a new terminal) over them, the safety rules, and the calls themselves. The registry
//! never stores a position: a dock tab can be reordered or closed by the user at any time, so
//! every call finds the terminal again by entity identity (`terminal_spot`) and forgets entries
//! whose terminal is gone.
//!
//! A free `command` (`terminals.open`) and typed `text` (`terminals.send`) wait for the user:
//! the call validates everything, then either runs at once (the Mono's remembered exact
//! string for this project, see `mono_approval.rs`) or returns a `Step::Ask` that the shell
//! queues as a dialog. Approving runs the very same function again with the request, so every
//! guard (terminal still there, owned by the Mono, not running an agent, folder inside the
//! project) is checked a second time right before anything is written, and what the dialog
//! showed (`Seen`: the program in front, the folder, bracketed paste) must still be true.
//! A saved Project Action never asks: the user wrote it.
//!
//! Keys: ctrl-c and q are always free. Up, down, tab and esc always wait for the user, whatever
//! holds the terminal (they recall history and complete lines in a shell or a REPL, and the
//! process name cannot tell which one a wrapper such as `uv run python` runs); the dialog
//! shows the last lines on screen and is run once, never remembered. Enter is free in front
//! of a plain program, else only with approved text in the same call (see `key_refusal`).
use super::*;
use crate::{
    pane_layout::{Edge, Preset, preset_layout},
    terminal::{Front, MonoKey, Terminal, tail_lines},
    workspace::{MonoTerminalMeta, SavedRun, TerminalPane},
};
use monocode_core::mono::api::is_format_char;
use serde_json::{Value, json};
use std::{
    path::{Component, Path},
    time::{Duration, Instant},
};

/// The most bytes of a free `command` or typed `text`.
const INPUT_MAX: usize = monocode_core::mono::api::TERMINAL_INPUT_MAX;
const LAYOUT_MAX: usize = monocode_core::mono::api::TERMINAL_LAYOUT_MAX;
const LABEL_MAX: usize = 40;
const READ_LINES_DEFAULT: usize = 200;
const READ_LINES_MAX: usize = 1000;
const READ_BYTES_MAX: usize = 32 * 1024;
/// How long `terminals.stop` waits after Ctrl-C before it hangs the job up.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// What a `terminals.*` call comes to: the answer now, or the answer once the user decides.
pub(crate) enum MonoTerminalReply {
    Now(Result<Value, String>),
    Later(async_channel::Receiver<Result<Value, String>>),
}

/// A checked call that needs the user's approval before it may write anything.
pub(super) struct Request {
    /// "command" (terminals.open) or "text" (terminals.send): which remembered list it is in.
    /// "key": a history key alone (terminals.send), which is never remembered.
    pub kind: &'static str,
    /// The exact string, as the Mono sent it.
    pub text: String,
    /// The terminal's name: the label the Mono asked for, else the terminal's current title.
    pub label: String,
    /// The folder shown: the terminal's live one when `cwd_live`, else where it started (a
    /// `cd` may have moved a shell since).
    pub cwd: String,
    pub cwd_live: bool,
    /// The terminal the call writes to, as the dialog showed it. `None`: a new terminal.
    pub seen: Option<Seen>,
    /// A key the call presses after the text (or, for kind "key", the key itself).
    pub then_key: Option<String>,
    /// The last lines on screen when a history key waits, as they were when asked.
    pub screen: Option<String>,
    /// The call to run again, once approved.
    pub input: Value,
    pub action: &'static str,
}

impl Request {
    /// Whether the dialog may offer "Allow ... for this Mono": not for a key, and not for text
    /// whose call also presses a key that needs approval each time.
    pub fn allowable(&self) -> bool {
        self.kind != "key"
            && !self
                .then_key
                .as_deref()
                .and_then(MonoKey::parse)
                .is_some_and(MonoKey::needs_approval)
    }
}

/// What the user was shown about a terminal. Writing is refused when any of it changed
/// between the dialog and the write: the approval was for that terminal in that state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Seen {
    pub front: Front,
    pub cwd: Option<String>,
    pub bracketed: bool,
}

impl Seen {
    /// Whether `now` is the state this was shown for. The paste mode only matters to a
    /// multi-line text.
    fn still(&self, now: &Seen, multiline: bool) -> bool {
        self.front == now.front
            && self.cwd == now.cwd
            && (!multiline || self.bracketed == now.bracketed)
    }

    fn of(t: &Terminal) -> Self {
        Self {
            front: t.live_front(),
            cwd: t.live_cwd(),
            bracketed: t.bracketed_paste(),
        }
    }
}

/// Who made a call: the Mono chat session, and when the bridge received it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Via {
    pub session: i64,
    pub received: Instant,
}

enum Step {
    Done(Value),
    Ask(Box<Request>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Origin {
    Mono,
    User,
}

impl Origin {
    fn name(self) -> &'static str {
        match self {
            Origin::Mono => "mono",
            Origin::User => "user",
        }
    }
}

struct Entry {
    id: String,
    origin: Origin,
    /// The Mono that opened it; `None` for a user terminal.
    mono: Option<String>,
    project: String,
    /// The saved Project Action it last ran, if any.
    action: Option<String>,
    terminal: gpui::WeakEntity<Terminal>,
}

pub(super) struct Registry {
    /// Random per launch: an id from an earlier run (a receipt, a note) names nothing now.
    prefix: String,
    next: u64,
    entries: Vec<Entry>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            prefix: monocode_core::mono::new_id()[..4].to_owned(),
            next: 0,
            entries: Vec::new(),
        }
    }
}

/// Where a terminal is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Dock(usize),
    Pane(i64),
}

struct Found {
    origin: Origin,
    terminal: Entity<Terminal>,
    spot: Spot,
}

/// Up to the last `max` bytes of `text`, cut at a character boundary.
fn cap_tail(text: String, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_owned(), true)
}

fn inside(root: &str, path: &str) -> bool {
    let (root, path) = (
        monocode_core::files::path_key(root),
        monocode_core::files::path_key(path),
    );
    path == root || path.starts_with(&format!("{}/", root.trim_end_matches('/')))
}

/// A folder's identity for comparing two spellings of it: symlinks resolved when it exists.
fn real_key(path: &str) -> String {
    let real = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned());
    monocode_core::files::path_key(&real)
}

fn project_worktrees(project: &str) -> Vec<String> {
    monocode_core::git::worktrees(Path::new(project))
        .map(|trees| trees.into_iter().map(|t| t.path).collect())
        .unwrap_or_default()
}

/// Whether a terminal's live folder is the project or inside it or one of its worktrees. A
/// folder that is gone, or reached through `..`, is not.
fn live_cwd_inside(project: &str, cwd: &str) -> bool {
    resolve_cwd(project, Some(cwd), || project_worktrees(project)).is_ok()
}

const CHANGED: &str = "The terminal changed after the dialog was shown (its program, folder or paste mode), so nothing was written. Ask again";
const OUTSIDE: &str = "The terminal's folder is outside the project and its worktrees, so Mono does not type into it. Close it, or open a new terminal with terminals.open";

/// Why `key` may not be pressed now, if it may not. Ctrl-C and q are always free. Up, down,
/// tab and esc never get here: they recall history, complete lines and start meta-key recalls
/// wherever a shell or a REPL may sit behind the program's name, so each press goes through
/// the approval dialog (`MonoKey::needs_approval`). Enter at a shell prompt runs whatever line
/// is there, so it only goes with text the user approved in the same call (the dialog then
/// says "presses enter"); it is free in front of a plain program. A shell, a remote or
/// multiplexed shell (ssh, tmux, sudo) or a REPL with history (python, psql) is not plain
/// (`Front::is_plain_program`).
fn key_refusal(key: MonoKey, front: &Front, with_text: bool) -> Option<&'static str> {
    match key {
        MonoKey::Enter if front.is_plain_program() || with_text => None,
        MonoKey::Enter => Some(
            "A shell is at its prompt: enter only goes with text the user approves in the same call (terminals.send text and key enter)",
        ),
        _ => None,
    }
}

/// The lines of the screen the approval dialog shows for a history key.
const SCREEN_LINES: usize = 3;
const SCREEN_COLUMNS: usize = 160;

/// The last lines of what the terminal shows, each cut to its last `SCREEN_COLUMNS`
/// characters, so the user sees what Enter would act on.
fn screen_tail(text: &str) -> String {
    let (tail, _) = tail_lines(text.trim_end(), SCREEN_LINES);
    tail.split('\n')
        .map(|line| {
            let line = line.trim_end();
            let count = line.chars().count();
            if count > SCREEN_COLUMNS {
                let cut: String = line.chars().skip(count - SCREEN_COLUMNS).collect();
                format!("...{cut}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a terminal with this front is idle enough for `terminals.open` to reuse it.
/// When the system cannot tell who holds it (`Unknown`), the last poll decides on unix (else
/// every call would open a duplicate). Off unix the poll never sees anything, so "no program"
/// proves nothing and a running program would be taken for an idle shell: it is not reused,
/// and a new terminal opens under the same label (terminals are found by id, never by label).
/// Writing treats `Unknown` as not a shell either way.
fn idle_for_reuse(front: &Front, poll_idle: bool, unix: bool) -> bool {
    match front {
        Front::Shell(_) => true,
        Front::Unknown => unix && poll_idle,
        Front::Program(_) => false,
    }
}

/// Multi-line input is one bracketed paste when the program asked for that, else each line
/// would run as it arrives: refused.
fn multiline_refusal(text: &str, bracketed: bool) -> Option<&'static str> {
    (text.contains('\n') && !bracketed).then_some(
        "This terminal is not in bracketed paste mode, so a line break would run each line as it arrives. Send one line at a time",
    )
}

/// The directory a Mono terminal starts in: the project, or `requested` when it stays inside
/// the project or one of its worktrees (`worktrees` is asked only then). `..` is refused
/// outright and symlinks are resolved before the comparison.
fn resolve_cwd(
    project: &str,
    requested: Option<&str>,
    worktrees: impl FnOnce() -> Vec<String>,
) -> Result<String, String> {
    let Some(requested) = requested.map(str::trim) else {
        return Ok(project.to_owned());
    };
    if requested.is_empty() {
        return Err("cwd must not be empty".into());
    }
    let path = Path::new(requested);
    if path.components().any(|c| c == Component::ParentDir) {
        return Err("cwd must not contain ..".into());
    }
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(project).join(path)
    };
    let real = std::fs::canonicalize(&full)
        .map_err(|_| format!("cwd {} does not exist", full.display()))?;
    if !real.is_dir() {
        return Err("cwd is not a directory".into());
    }
    let real = real.to_string_lossy().into_owned();
    let within = |root: &str| {
        std::fs::canonicalize(root).is_ok_and(|root| inside(&root.to_string_lossy(), &real))
    };
    if within(project) || worktrees().iter().any(|tree| within(tree)) {
        return Ok(full.to_string_lossy().into_owned());
    }
    Err("cwd must be inside the project or one of its worktrees".into())
}

fn parse_label(input: &Value) -> Result<String, String> {
    let label = input
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if label.is_empty() || label.chars().any(is_format_char) {
        return Err(
            "Provide a label: one line of visible text (no control, zero-width or direction characters)"
                .into(),
        );
    }
    if label.chars().count() > LABEL_MAX {
        return Err(format!("label is at most {LABEL_MAX} characters"));
    }
    Ok(label.to_owned())
}

fn id_of(input: &Value) -> Result<String, String> {
    input
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Provide id: a terminal id from terminals.list or terminals.open".into())
}

/// `command` or `text`, checked before the user is asked: text, not empty, at most 4096
/// bytes, and no control character except line breaks. ESC, Tab, Enter (`\r`), Ctrl-C and
/// the rest are keys, never text, so a Mono cannot type an escape sequence or ask the shell to
/// complete.
fn parse_typed(input: &Value, field: &str) -> Result<Option<String>, String> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| format!("{field} must be text"))?;
    if text.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if text.len() > INPUT_MAX {
        return Err(format!("{field} is at most {INPUT_MAX} bytes"));
    }
    if text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!(
            "{field} must not contain control characters: use key for control keys ({})",
            MonoKey::NAMES
        ));
    }
    Ok(Some(text.to_owned()))
}

impl Shell {
    pub(crate) fn mono_terminals_call(
        &mut self,
        mono: &str,
        project: &str,
        action: &str,
        input: &Value,
        via: Via,
        cx: &mut Context<Self>,
    ) -> MonoTerminalReply {
        let project = self.workspace_project_key(project);
        let project = project.as_str();
        match self.mono_terminals_step(mono, project, action, input, None, cx) {
            Ok(Step::Done(value)) => MonoTerminalReply::Now(Ok(value)),
            Ok(Step::Ask(request)) => self.queue_mono_approval(mono, project, *request, via, cx),
            Err(error) => MonoTerminalReply::Now(Err(error)),
        }
    }

    /// Runs a call that was approved: the same checks as the call itself, now allowed to write,
    /// and only into the terminal in the state the dialog showed.
    pub(super) fn mono_terminals_approved(
        &mut self,
        mono: &str,
        project: &str,
        request: &Request,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let project = self.workspace_project_key(project);
        let project = project.as_str();
        let step = self.mono_terminals_step(
            mono,
            project,
            request.action,
            &request.input,
            Some(request),
            cx,
        )?;
        match step {
            Step::Done(value) => Ok(value),
            Step::Ask(_) => Err("The command was not approved".into()),
        }
    }

    fn mono_terminals_step(
        &mut self,
        mono: &str,
        project: &str,
        action: &str,
        input: &Value,
        approved: Option<&Request>,
        cx: &mut Context<Self>,
    ) -> Result<Step, String> {
        if project.starts_with("remote://") {
            return Err("Terminals are not available on remote projects".into());
        }
        self.prune_mono_terminals();
        self.restore_mono_registry(project, cx);
        let done = |value| Ok(Step::Done(value));
        match action {
            "terminals.list" => done(self.mono_list_terminals(mono, project, cx)),
            "terminals.open" => self.mono_open_terminal(mono, project, input, approved, cx),
            "terminals.send" => self.mono_send_terminal(mono, project, input, approved, cx),
            "terminals.read" => self
                .mono_read_terminal(mono, project, input, cx)
                .and_then(done),
            "terminals.stop" => self
                .mono_stop_terminal(mono, project, input, cx)
                .and_then(done),
            "terminals.close" => self
                .mono_close_terminal(mono, project, input, cx)
                .and_then(done),
            "terminals.layout" => self
                .mono_layout_terminals(mono, project, input, cx)
                .and_then(done),
            _ => Err("Unknown terminals action".into()),
        }
    }

    /// The key the workspace already knows `project` by (its current project, a dock, a live
    /// terminal list or a session's project), so a path spelled with a trailing slash or other
    /// case-insensitive form addresses the same docks and terminals as the user's own.
    fn workspace_project_key(&self, project: &str) -> String {
        let key = monocode_core::files::path_key(project);
        let sessions = self
            .session_summaries
            .iter()
            .map(|(id, s)| self.project_of(*id, &s.cwd).to_owned());
        std::iter::once(self.workspace.project.clone())
            .chain(self.workspace.docks.keys().cloned())
            .chain(self.terminals.keys().cloned())
            .chain(sessions)
            .find(|known| monocode_core::files::path_key(known) == key)
            .unwrap_or_else(|| {
                let trimmed = project.trim_end_matches('/');
                if trimmed.is_empty() { project } else { trimmed }.to_owned()
            })
    }

    /// Whether a queued call could still run: the checks it makes before writing, without
    /// writing. The shell asks again every half second while the dialog waits, and once more
    /// when the user approves.
    pub(super) fn mono_terminal_ready(
        &self,
        mono: &str,
        project: &str,
        action: &str,
        input: &Value,
        cx: &App,
    ) -> Result<(), String> {
        match action {
            "terminals.send" => {
                let found = self.find_own_terminal(mono, project, &id_of(input)?)?;
                let t = found.terminal.read(cx);
                if t.exited {
                    return Err("The terminal has exited".into());
                }
                if t.agent().is_some() {
                    return Err(
                        "This terminal runs an agent CLI; Mono never types into those".into(),
                    );
                }
                Ok(())
            }
            _ => {
                resolve_cwd(project, input.get("cwd").and_then(Value::as_str), || {
                    monocode_core::git::worktrees(Path::new(project))
                        .map(|trees| trees.into_iter().map(|t| t.path).collect())
                        .unwrap_or_default()
                })?;
                if let Some(id) = input.get("beside").and_then(Value::as_str) {
                    self.find_beside(mono, project, id)?;
                }
                Ok(())
            }
        }
    }

    /// Whether the user let this Mono run exactly `text` as `kind` in this project without
    /// asking.
    fn mono_allows(&self, mono: &str, project: &str, kind: &str, text: &str, cx: &App) -> bool {
        self.monos
            .as_ref()
            .is_some_and(|monos| monos.read(cx).terminal_allowed(mono, project, kind, text))
    }

    /// The project a terminal pane belongs to: its owner session's.
    fn terminal_pane_project(&self, pane: i64) -> Option<String> {
        let owner = self.workspace.terminal_panes.get(&pane)?.owner;
        let summary = self.session_summaries.get(&owner)?;
        Some(self.project_of(owner, &summary.cwd).to_owned())
    }

    /// Where a terminal is, as `terminals.list` and `terminals.open` both say it: "dock", "tab"
    /// (a pane alone in its title tab) or "split" (a pane sharing its tab).
    fn place_name(&self, spot: Spot) -> &'static str {
        match spot {
            Spot::Dock(_) => "dock",
            Spot::Pane(pane) => {
                let split = self
                    .workspace
                    .tab_of(pane)
                    .is_some_and(|tab| self.workspace.layout(tab).is_split());
                if split { "split" } else { "tab" }
            }
        }
    }

    fn terminal_spot(&self, project: &str, terminal: &Entity<Terminal>) -> Option<Spot> {
        if let Some(ix) = self
            .terminals
            .get(project)
            .and_then(|live| live.iter().position(|t| t == terminal))
        {
            return Some(Spot::Dock(ix));
        }
        self.terminal_pane_views
            .iter()
            .find(|(pane, view)| {
                *view == terminal
                    && self.terminal_pane_project(**pane).is_some_and(|p| {
                        monocode_core::files::path_key(&p)
                            == monocode_core::files::path_key(project)
                    })
            })
            .map(|(pane, _)| Spot::Pane(*pane))
    }

    /// Forgets terminals the user closed.
    fn prune_mono_terminals(&mut self) {
        let mut registry = std::mem::take(&mut self.mono_registry);
        registry.entries.retain(|entry| {
            entry
                .terminal
                .upgrade()
                .is_some_and(|t| self.terminal_spot(&entry.project, &t).is_some())
        });
        self.mono_registry = registry;
    }

    fn registry_id(
        &mut self,
        terminal: &Entity<Terminal>,
        origin: Origin,
        mono: Option<&str>,
        project: &str,
    ) -> String {
        let registry = &mut self.mono_registry;
        if let Some(entry) = registry
            .entries
            .iter()
            .find(|e| e.terminal.entity_id() == terminal.entity_id())
        {
            return entry.id.clone();
        }
        registry.next += 1;
        let id = format!("t-{}-{}", registry.prefix, registry.next);
        registry.entries.push(Entry {
            id: id.clone(),
            origin,
            mono: mono.map(str::to_owned),
            project: project.to_owned(),
            action: None,
            terminal: terminal.downgrade(),
        });
        id
    }

    /// A terminal this Mono may address: its own, or a user's of the same project. Another
    /// Mono's terminals and anything not in the registry are unknown.
    fn find_mono_terminal(&self, mono: &str, project: &str, id: &str) -> Result<Found, String> {
        let unknown = || format!("Unknown terminal {id}. Run terminals.list");
        let entry = self
            .mono_registry
            .entries
            .iter()
            .find(|e| e.id == id)
            .filter(|e| {
                monocode_core::files::path_key(&e.project)
                    == monocode_core::files::path_key(project)
                    && (e.origin == Origin::User || e.mono.as_deref() == Some(mono))
            })
            .ok_or_else(unknown)?;
        let terminal = entry.terminal.upgrade().ok_or_else(unknown)?;
        let spot = self.terminal_spot(project, &terminal).ok_or_else(unknown)?;
        Ok(Found {
            origin: entry.origin,
            terminal,
            spot,
        })
    }

    /// Like `find_mono_terminal`, for what only the terminal's opener may do.
    fn find_own_terminal(&self, mono: &str, project: &str, id: &str) -> Result<Found, String> {
        let found = self.find_mono_terminal(mono, project, id)?;
        if found.origin != Origin::Mono {
            return Err(
                "A terminal the user opened cannot be typed into, stopped or closed".into(),
            );
        }
        Ok(found)
    }

    /// The terminal `beside` names: one of this Mono's own. A user's terminal pane may sit in
    /// the user's chat tab, and a Mono never splits that.
    fn find_beside(&self, mono: &str, project: &str, id: &str) -> Result<Found, String> {
        self.find_own_terminal(mono, project, id).map_err(|_| {
            "beside must be a terminal this Mono opened (see terminals.list); the user's terminals are never split".to_owned()
        })
    }

    fn mono_list_terminals(&mut self, mono: &str, project: &str, cx: &App) -> Value {
        let mut spots: Vec<(Entity<Terminal>, Spot)> = self
            .terminals
            .get(project)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(ix, t)| (t.clone(), Spot::Dock(ix)))
            .collect();
        let mut panes: Vec<_> = self
            .terminal_pane_views
            .iter()
            .filter(|(pane, _)| {
                self.terminal_pane_project(**pane).is_some_and(|p| {
                    monocode_core::files::path_key(&p) == monocode_core::files::path_key(project)
                })
            })
            .map(|(pane, view)| (view.clone(), Spot::Pane(*pane)))
            .collect();
        // Newer panes have lower ids.
        panes.sort_by_key(|(_, spot)| match spot {
            Spot::Pane(pane) => -pane,
            Spot::Dock(_) => 0,
        });
        spots.extend(panes);
        let mut rows = Vec::new();
        for (terminal, spot) in spots {
            if !terminal.read(cx).is_plain_shell() {
                continue;
            }
            let known = self
                .mono_registry
                .entries
                .iter()
                .find(|e| e.terminal.entity_id() == terminal.entity_id())
                .map(|e| (e.origin, e.mono.clone()));
            // Another Mono's terminals are not this one's to see.
            if matches!(&known, Some((Origin::Mono, m)) if m.as_deref() != Some(mono)) {
                continue;
            }
            let origin = known.map_or(Origin::User, |(origin, _)| origin);
            let id = self.registry_id(&terminal, origin, None, project);
            let t = terminal.read(cx);
            let place = self.place_name(spot);
            rows.push(json!({
                "id": id,
                "label": t.display_title(),
                "cwd": t.cwd,
                "place": place,
                "running": t.foreground.is_some() && !t.exited,
                "exited": t.exited,
                "exitCode": t.exit_code,
                "owner": origin.name(),
            }));
        }
        Value::Array(rows)
    }

    /// The ONE place a command a Mono terminal runs reaches its shell: a saved Project Action
    /// (the user wrote it), a free command the user approved (dialog or allow-list), or the
    /// command the user pressed "Run again" for. Callers have already checked who may run
    /// what; nothing else types a command into a Mono terminal. Keeps the command as the
    /// terminal's last run, saved with the workspace, and hides the "Run again" bar.
    fn run_mono_input(
        &mut self,
        terminal: &Entity<Terminal>,
        run: SavedRun,
        cx: &mut Context<Self>,
    ) {
        terminal.update(cx, |t, cx| {
            t.run_again = None;
            if run.kind == "action" {
                t.run(&run.text, cx);
            } else {
                t.send_text(&run.text, true, cx);
            }
        });
        if let Some(entry) = self
            .mono_registry
            .entries
            .iter_mut()
            .find(|e| e.terminal.entity_id() == terminal.entity_id())
        {
            entry.action = run.action.clone();
        }
        match self.locate_terminal(terminal) {
            Some((_, Spot::Pane(pane))) => {
                if let Some(state) = self.workspace.terminal_panes.get_mut(&pane) {
                    state.run = Some(run);
                }
            }
            Some((project, Spot::Dock(ix))) => {
                if let Some(dock) = self.workspace.docks.get_mut(&project)
                    && let Some(mut meta) = dock.mono_meta(ix).cloned()
                {
                    meta.run = Some(run);
                    dock.set_mono_meta(ix, Some(meta));
                }
            }
            None => {}
        }
        self.persist(cx);
    }

    /// The user pressed "Run again" on a restored Mono terminal. The press is the approval,
    /// so no dialog; it still runs through `run_mono_input`.
    pub(super) fn run_again_clicked(
        &mut self,
        terminal: &Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        let run = {
            let t = terminal.read(cx);
            if t.exited {
                return;
            }
            t.run_again.clone()
        };
        let Some(run) = run else {
            return;
        };
        // What the bar showed must be exactly what runs. A saved action edited since the
        // bar was drawn shows its current command first; the next press runs that one.
        let current = self.locate_terminal(terminal).map_or_else(
            || run.clone(),
            |(project, _)| self.current_run(&project, &run),
        );
        if current != run {
            terminal.update(cx, |t, cx| {
                t.run_again = Some(current);
                cx.notify();
            });
            return;
        }
        self.run_mono_input(terminal, run, cx);
    }

    /// `run` with the saved action's current command, when it names an action that still
    /// exists (else the text as stored).
    fn current_run(&self, project: &str, run: &SavedRun) -> SavedRun {
        run.action
            .as_deref()
            .filter(|_| run.kind == "action")
            .and_then(|id| {
                self.workspace
                    .actions
                    .get(project)?
                    .iter()
                    .find(|action| action.id == id)
            })
            .map_or_else(
                || run.clone(),
                |action| SavedRun {
                    text: action.command.clone(),
                    ..run.clone()
                },
            )
    }

    /// The project and place of a terminal, wherever it is.
    fn locate_terminal(&self, terminal: &Entity<Terminal>) -> Option<(String, Spot)> {
        for (project, live) in &self.terminals {
            if let Some(ix) = live.iter().position(|t| t == terminal) {
                return Some((project.clone(), Spot::Dock(ix)));
            }
        }
        let (pane, _) = self
            .terminal_pane_views
            .iter()
            .find(|(_, view)| *view == terminal)?;
        Some((self.terminal_pane_project(*pane)?, Spot::Pane(*pane)))
    }

    /// What a restored Mono terminal wears: its label, the Mono mark and the "Run again" bar.
    /// Runs nothing.
    pub(super) fn restore_mono_terminal(
        terminal: &Entity<Terminal>,
        label: Option<String>,
        run: Option<SavedRun>,
        cx: &mut App,
    ) {
        terminal.update(cx, |t, _| {
            t.label = label;
            t.by_mono = true;
            t.run_again = run;
        });
    }

    /// After a restart the project's saved Mono terminals are shells again, with no registry
    /// entry. Gives each its entry (new id, same label, Mono and project) so the Mono finds it
    /// with `terminals.list` and reuses it by label. Nothing runs and nothing is approved.
    fn restore_mono_registry(&mut self, project: &str, cx: &mut Context<Self>) {
        let key = monocode_core::files::path_key(project);
        let dock_metas: Vec<(usize, MonoTerminalMeta)> = self
            .workspace
            .docks
            .get(project)
            .map(|dock| {
                dock.mono_terminals
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, meta)| Some((ix, meta.clone()?)))
                    .collect()
            })
            .unwrap_or_default();
        let panes: Vec<i64> = self
            .workspace
            .terminal_panes
            .iter()
            .filter(|(pane, state)| {
                state.mono.is_some()
                    && self
                        .terminal_pane_project(**pane)
                        .is_some_and(|p| monocode_core::files::path_key(&p) == key)
            })
            .map(|(pane, _)| *pane)
            .collect();
        if dock_metas.is_empty() && panes.is_empty() {
            return;
        }
        let mut restored: Vec<(Entity<Terminal>, String, Option<String>)> = Vec::new();
        if let Some(last) = dock_metas.iter().map(|(ix, _)| *ix).max() {
            // The live shells are a prefix of the dock's tabs: only those up to the last Mono
            // terminal are spawned, so the user's tabs after it stay unspawned.
            self.ensure_terminals_through(project, last, cx);
            for (ix, meta) in dock_metas {
                if let Some(t) = self.terminals.get(project).and_then(|live| live.get(ix)) {
                    restored.push((t.clone(), meta.mono, meta.run.and_then(|r| r.action)));
                }
            }
        }
        for pane in panes {
            let (mono, action) = {
                let state = &self.workspace.terminal_panes[&pane];
                (
                    state.mono.clone().unwrap_or_default(),
                    state.run.as_ref().and_then(|r| r.action.clone()),
                )
            };
            if let Some(t) = self.terminal_pane_view(pane, cx) {
                restored.push((t, mono, action));
            }
        }
        for (terminal, mono, action) in restored {
            let known = self
                .mono_registry
                .entries
                .iter()
                .any(|e| e.terminal.entity_id() == terminal.entity_id());
            if known {
                continue;
            }
            let id = self.registry_id(&terminal, Origin::Mono, Some(&mono), project);
            if let Some(entry) = self.mono_registry.entries.iter_mut().find(|e| e.id == id) {
                entry.action = action;
            }
        }
    }

    fn mono_open_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        approved: Option<&Request>,
        cx: &mut Context<Self>,
    ) -> Result<Step, String> {
        let label = parse_label(input)?;
        let command = parse_typed(input, "command")?;
        if command.is_some() && input.get("action").is_some() {
            return Err("Give action or command, not both".into());
        }
        let action = match input.get("action") {
            None => None,
            Some(id) => {
                let id = id.as_str().ok_or("action must be a saved action's id")?;
                let known = self.workspace.actions.get(project);
                let action = known
                    .and_then(|all| all.iter().find(|a| a.id == id))
                    .cloned()
                    .ok_or_else(|| {
                        let ids = known
                            .map(|all| {
                                all.iter()
                                    .map(|a| format!("{} ({})", a.id, a.name))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default();
                        format!("Unknown action {id}. Saved actions: [{ids}]")
                    })?;
                Some(action)
            }
        };
        if input.get("cwd").is_some_and(|v| !v.is_string()) {
            return Err("cwd must be text".into());
        }
        let cwd = resolve_cwd(project, input.get("cwd").and_then(Value::as_str), || {
            project_worktrees(project)
        })?;
        let place = match input.get("place").and_then(Value::as_str) {
            None => None,
            Some(place @ ("dock" | "split")) => Some(place),
            Some(_) => return Err("place must be \"dock\" or \"split\"".into()),
        };
        let edge = match input.get("direction").and_then(Value::as_str) {
            None | Some("right") => Edge::Right,
            Some("down") => Edge::Bottom,
            Some(_) => return Err("direction must be \"right\" or \"down\"".into()),
        };
        let (reveal, switch) = self.reveal_mode(project, input);
        // The tab the user is looking at now, read before anything is placed or selected.
        let from = self.workspace.active_tab();
        let beside = match input.get("beside").and_then(Value::as_str) {
            None => None,
            Some(id) => Some(self.find_beside(mono, project, id)?),
        };
        let run = match (&action, &command) {
            (Some(action), _) => Some(SavedRun {
                kind: "action".into(),
                text: action.command.clone(),
                action: Some(action.id.clone()),
            }),
            (None, Some(command)) => Some(SavedRun {
                kind: "command".into(),
                text: command.clone(),
                action: None,
            }),
            (None, None) => None,
        };
        // An idle terminal with this label is reused, like an action's own terminal, when it
        // is the Mono's, stands at its shell prompt right now (not as of the last poll) and is
        // in the folder asked for (the project's own when none was). Where it stands is asked
        // of the system when it can tell, else it is where it started.
        let wanted = real_key(&cwd);
        let reusable = self.mono_registry.entries.iter().find_map(|entry| {
            // The cheap tests first: the system is asked about a terminal only when it is this
            // Mono's, in this project, with this label.
            if entry.origin != Origin::Mono
                || entry.mono.as_deref() != Some(mono)
                || monocode_core::files::path_key(&entry.project)
                    != monocode_core::files::path_key(project)
            {
                return None;
            }
            let terminal = entry.terminal.upgrade()?;
            let t = terminal.read(cx);
            if t.label.as_deref() != Some(label.as_str()) || t.exited {
                return None;
            }
            let seen = Seen::of(t);
            let at_prompt = idle_for_reuse(&seen.front, t.foreground.is_none(), cfg!(unix));
            let there = seen.cwd.clone().unwrap_or_else(|| t.cwd.clone());
            let same = at_prompt
                && real_key(&there) == wanted
                && seen
                    .cwd
                    .as_deref()
                    .is_none_or(|live| live_cwd_inside(project, live));
            same.then(|| (entry.id.clone(), terminal.clone(), seen))
        });
        let reusable =
            reusable.filter(|(_, terminal, _)| self.terminal_spot(project, terminal).is_some());
        let seen = reusable.as_ref().map(|(_, _, seen)| seen.clone());
        if let Some(command) = &command {
            // A multi-line command is a paste, which only a shell that asked for one takes.
            // A terminal that is not there yet has asked for nothing.
            if let Some(refusal) =
                multiline_refusal(command, seen.as_ref().is_some_and(|s| s.bracketed))
            {
                return Err(refusal.into());
            }
            match approved {
                Some(request) => {
                    let same = match (&request.seen, &seen) {
                        (None, None) => true,
                        (Some(shown), Some(now)) => shown.still(now, command.contains('\n')),
                        _ => false,
                    };
                    if !same {
                        return Err(CHANGED.into());
                    }
                }
                None if !self.mono_allows(mono, project, "command", command, cx) => {
                    return Ok(Step::Ask(Box::new(Request {
                        kind: "command",
                        text: command.clone(),
                        label,
                        cwd_live: seen.as_ref().is_some_and(|s| s.cwd.is_some()),
                        cwd: seen
                            .as_ref()
                            .and_then(|s| s.cwd.clone())
                            .unwrap_or_else(|| cwd.clone()),
                        seen,
                        then_key: None,
                        screen: None,
                        input: input.clone(),
                        action: "terminals.open",
                    })));
                }
                None => {}
            }
        }
        if let Some((id, terminal, _)) = reusable
            && let Some(spot) = self.terminal_spot(project, &terminal)
        {
            if let Some(run) = run.clone() {
                self.run_mono_input(&terminal, run, cx);
            }
            let revealed = !reveal || self.reveal_mono_terminal(project, spot, switch, from, cx);
            let mut done = json!({"id": id, "reused": true, "place": self.place_name(spot)});
            if !revealed {
                done["revealed"] = json!(false);
            }
            return Ok(Step::Done(done));
        }

        let in_panel = self.terminal_in_panel(cx);
        let split = match place {
            Some(place) => place == "split",
            None => self.workspace.preferences.terminal_placement() == "title" && !in_panel,
        };
        let mut opened = None;
        if split {
            let target = self.mono_split_target(mono, project, beside.as_ref())?;
            opened = self.open_mono_terminal_pane(
                mono, project, &label, &cwd, target, edge, reveal, switch, cx,
            );
        } else if let Some(beside) = &beside
            && matches!(beside.spot, Spot::Pane(_))
        {
            // Beside a pane means a pane, whatever the user's default says.
            let target = self.mono_split_target(mono, project, Some(beside))?;
            opened = self.open_mono_terminal_pane(
                mono, project, &label, &cwd, target, edge, reveal, switch, cx,
            );
        }
        let (terminal, spot) = match opened {
            Some(opened) => opened,
            None => self.open_mono_dock_terminal(mono, project, &label, &cwd, reveal, in_panel, cx),
        };
        let id = self.registry_id(&terminal, Origin::Mono, Some(mono), project);
        if let Some(run) = run {
            self.run_mono_input(&terminal, run, cx);
        }
        let revealed = !reveal || self.reveal_mono_terminal(project, spot, switch, from, cx);
        cx.notify();
        let mut done = json!({"id": id, "reused": false, "place": self.place_name(spot)});
        if !revealed {
            done["revealed"] = json!(false);
        }
        Ok(Step::Done(done))
    }

    /// How a call's `reveal` reads, as `(reveal, switch)`. Omitted, it shows the terminal only
    /// where the user is looking and only without navigating: the tab it is in must already
    /// be the visible one. Only an explicit `true` may select another title tab (`switch`).
    fn reveal_mode(&self, project: &str, input: &Value) -> (bool, bool) {
        let explicit = input.get("reveal").and_then(Value::as_bool);
        let in_view = monocode_core::files::path_key(project)
            == monocode_core::files::path_key(&self.workspace.project);
        (explicit.unwrap_or(in_view), explicit == Some(true))
    }

    /// The pane a new split terminal goes beside: `beside`, else the Mono's Logs tab of the
    /// project. `None` opens a title tab of its own (a Logs tab of one, which later splits
    /// join); the user's own tabs are never split.
    fn mono_split_target(
        &self,
        mono: &str,
        project: &str,
        beside: Option<&Found>,
    ) -> Result<Option<i64>, String> {
        let in_project = |pane: i64| {
            let session = self.workspace.session_for_pane(pane);
            self.session_summaries.get(&session).is_some_and(|s| {
                monocode_core::files::path_key(self.project_of(session, &s.cwd))
                    == monocode_core::files::path_key(project)
            })
        };
        if let Some(beside) = beside {
            return match beside.spot {
                Spot::Pane(pane) if self.workspace.tab_of(pane).is_some() => Ok(Some(pane)),
                _ => Err("beside is a terminal in the dock; it has no pane to split".into()),
            };
        }
        // The Mono's Logs tab of this project takes new split terminals.
        let logs_name = logs_title(project);
        if let Some(tab) = self.workspace.logs_tab(mono, &logs_name, in_project) {
            return Ok(Some(self.workspace.layout(tab).first_leaf()));
        }
        // Otherwise a Logs tab of its own: the user's chat tab is never split by a Mono.
        Ok(None)
    }

    /// A new terminal pane. `None` when the project has no session to lend its scope: the
    /// caller then falls back to the dock.
    #[allow(clippy::too_many_arguments)]
    fn open_mono_terminal_pane(
        &mut self,
        mono: &str,
        project: &str,
        label: &str,
        cwd: &str,
        target: Option<i64>,
        edge: Edge,
        reveal: bool,
        switch: bool,
        cx: &mut Context<Self>,
    ) -> Option<(Entity<Terminal>, Spot)> {
        let owner = self.mono_pane_owner(project, None)?;
        let terminal = self.spawn_terminal_as(cwd.to_owned(), true, cx);
        terminal.update(cx, |t, _| {
            t.label = Some(label.to_owned());
            t.by_mono = true;
        });
        let pane = self.workspace.next_pane_id();
        let mut state = TerminalPane::new(owner, cwd.to_owned());
        state.mono = Some(mono.to_owned());
        state.label = Some(label.to_owned());
        // Joining the Mono's Logs tab keeps it a Logs tab, and a tab of its own is one.
        state.logs = match target.filter(|t| self.workspace.tab_of(*t).is_some()) {
            None => Some(logs_title(project)),
            Some(target) => self
                .workspace
                .terminal_panes
                .get(&target)
                .filter(|p| p.mono.as_deref() == Some(mono))
                .and_then(|_| self.workspace.tab_of(target))
                .and_then(|tab| self.workspace.logs_title(tab))
                .map(str::to_owned),
        };
        self.workspace.terminal_panes.insert(pane, state);
        self.terminal_pane_views.insert(pane, terminal.clone());
        let previous = self.workspace.active;
        // The new pane takes the view only where nothing is left behind: a visible tab, or an
        // explicit `reveal: true`. A tab of its own just joins the strip.
        let joins_visible = target.is_some_and(|t| {
            self.workspace.tab_of(t).is_some()
                && self.workspace.tab_of(t) == self.workspace.active_tab()
        });
        let activate = reveal && (switch || joins_visible);
        match target.filter(|t| self.workspace.tab_of(*t).is_some()) {
            Some(target) => {
                let animate = self.workspace.tab_of(target) == self.workspace.active_tab();
                let before = self.active_leaves();
                self.workspace.place(pane, target, edge);
                if animate {
                    self.begin_pane_motion(before, cx);
                } else {
                    self.pane_motion = None;
                }
            }
            None => {
                self.workspace.open(pane);
                self.pane_motion = None;
            }
        }
        // The user's pane stays the active one: opening is not navigating.
        if !activate {
            self.workspace.active = previous;
            if let Some(previous) = previous
                && let Some(tab) = self.workspace.tab_of(previous)
                && self.workspace.layouts.contains_key(&tab)
            {
                self.workspace.focused.insert(tab, previous);
            }
        }
        self.fit_sidebar(cx);
        self.persist(cx);
        Some((terminal, Spot::Pane(pane)))
    }

    /// A session of the project that lends a Mono's terminal pane its scope: the project's
    /// focused one, else any open one, else any in the sidebar. Never the Mono's own chat,
    /// a hidden session that does not live in the project.
    ///
    /// Only a local session qualifies: a cloud or remote one would bind the Files and Git
    /// panels to its workspace and could wake it. `skip` is a session on its way out.
    fn mono_pane_owner(&self, project: &str, skip: Option<i64>) -> Option<i64> {
        let key = monocode_core::files::path_key(project);
        let known = |id: i64| {
            self.session_summaries
                .get(&id)
                .filter(|s| {
                    Some(id) != skip
                        && !self.cloud_marks.contains_key(&id)
                        && !self.remote_marks.contains_key(&id)
                        && !s.cwd.starts_with("remote://")
                        && monocode_core::files::path_key(self.project_of(id, &s.cwd)) == key
                })
                .map(|_| id)
        };
        self.workspace
            .project_focus
            .get(project)
            .and_then(|pane| known(self.workspace.session_for_pane(*pane)))
            .or_else(|| {
                self.workspace
                    .open_sessions()
                    .into_iter()
                    .find_map(|id| known(self.workspace.session_for_pane(id)))
            })
            .or_else(|| self.order.iter().find_map(|id| known(*id)))
    }

    #[allow(clippy::too_many_arguments)]
    fn open_mono_dock_terminal(
        &mut self,
        mono: &str,
        project: &str,
        label: &str,
        cwd: &str,
        reveal: bool,
        in_panel: bool,
        cx: &mut Context<Self>,
    ) -> (Entity<Terminal>, Spot) {
        self.ensure_terminals(project, cx);
        self.cancel_dock_gestures(project, cx);
        let viewport = self.viewport();
        let dock = self.workspace.docks.entry(project.to_owned()).or_default();
        let before = (!dock.terminals.is_empty()).then_some((dock.active, dock.open));
        dock.size = dock.side.clamp(dock.size, viewport);
        let ix = dock.add(cwd.to_owned());
        dock.set_mono_meta(
            ix,
            Some(MonoTerminalMeta {
                mono: mono.to_owned(),
                label: label.to_owned(),
                run: None,
            }),
        );
        // `add` selects and opens; a background open leaves the dock as the user had it.
        if !reveal {
            match before {
                Some((active, open)) => {
                    dock.active = active;
                    dock.open = open;
                }
                None => dock.open = false,
            }
        }
        // The panel shows the terminals instead of the dock.
        if in_panel {
            dock.open = false;
        }
        let terminal = self.spawn_terminal_as(cwd.to_owned(), true, cx);
        terminal.update(cx, |t, _| {
            t.label = Some(label.to_owned());
            t.by_mono = true;
        });
        self.terminals
            .entry(project.to_owned())
            .or_default()
            .push(terminal.clone());
        self.persist(cx);
        (terminal, Spot::Dock(ix))
    }

    /// Shows a terminal without taking the keyboard or leaving a Mono's view. Only where the
    /// user is looking: another project's dock or tabs are not in view.
    /// Returns whether it is shown: not for another project, nor for a dock terminal while the
    /// terminal lives in the right panel (opening the panel takes a window and the keyboard).
    ///
    /// A pane in a tab that is not the visible one is shown only when `switch` (an explicit
    /// `reveal: true`) allows selecting that tab; the keyboard then goes to the shell root,
    /// unless what had it is still on screen (`focus_after_switch`). `from` is the visible
    /// tab before the call placed or selected anything.
    fn reveal_mono_terminal(
        &mut self,
        project: &str,
        spot: Spot,
        switch: bool,
        from: Option<i64>,
        cx: &mut Context<Self>,
    ) -> bool {
        if monocode_core::files::path_key(project)
            != monocode_core::files::path_key(&self.workspace.project)
        {
            return false;
        }
        match spot {
            Spot::Dock(ix) => {
                let in_panel = self.terminal_in_panel(cx);
                let Some(dock) = self.workspace.docks.get_mut(project) else {
                    return false;
                };
                let was_open = dock.open;
                dock.active = ix.min(dock.terminals.len().saturating_sub(1));
                dock.open |= !in_panel;
                let opened = dock.open && !was_open;
                self.dock_tab_scroll.scroll_to_item(ix);
                if opened {
                    self.slide_dock(true, cx);
                    self.fit_sidebar(cx);
                }
                self.persist(cx);
                cx.notify();
                !in_panel
            }
            Spot::Pane(pane) => {
                let tab = self.workspace.tab_of(pane);
                if !switch && (tab.is_none() || tab != from) {
                    return false;
                }
                self.reveal_terminal_pane(pane, cx);
                if self.workspace.active_tab() != from {
                    self.mono_approvals.switched = true;
                }
                true
            }
        }
    }

    /// `terminals.layout`: the given terminals of this Mono in one "Logs <project>" tab.
    fn mono_layout_terminals(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let preset = input
            .get("preset")
            .and_then(Value::as_str)
            .and_then(Preset::parse)
            .ok_or("preset must be \"columns\", \"rows\" or \"grid\"")?;
        let ids: Vec<&str> = input
            .get("ids")
            .and_then(Value::as_array)
            .and_then(|ids| ids.iter().map(Value::as_str).collect())
            .ok_or("ids must be a list of terminal ids")?;
        if ids.is_empty() || ids.len() > LAYOUT_MAX {
            return Err(format!("ids holds 1 to {LAYOUT_MAX} terminal ids"));
        }
        for (ix, id) in ids.iter().enumerate() {
            if ids[..ix].contains(id) {
                return Err("ids must not repeat a terminal".into());
            }
        }
        // Everything is checked before anything moves.
        let mut terminals = Vec::new();
        for id in &ids {
            terminals.push(self.find_own_terminal(mono, project, id)?.terminal);
        }
        let owner = self.mono_pane_owner(project, None);
        if owner.is_none()
            && terminals
                .iter()
                .any(|t| matches!(self.terminal_spot(project, t), Some(Spot::Dock(_))))
        {
            return Err("The project has no session to hold terminal panes".into());
        }
        let (reveal, switch) = self.reveal_mode(project, input);
        let from = self.workspace.active_tab();
        let title = logs_title(project);
        let mut panes = Vec::new();
        for terminal in &terminals {
            let pane = match self.terminal_spot(project, terminal) {
                Some(Spot::Pane(pane)) => pane,
                Some(Spot::Dock(ix)) => {
                    let owner = owner.expect("checked above");
                    // A drag or an open tab menu of the dock names tabs by position.
                    self.cancel_dock_gestures(project, cx);
                    self.move_dock_terminal_to_pane(project, ix, terminal.clone(), owner, cx)
                }
                None => return Err("A terminal went away; run terminals.list".into()),
            };
            let label = terminal.read(cx).label.clone();
            if let Some(state) = self.workspace.terminal_panes.get_mut(&pane) {
                state.mono = Some(mono.to_owned());
                state.label = state.label.take().or(label);
            }
            panes.push(pane);
        }
        let reuse = self.workspace.logs_tab(mono, &title, |pane| {
            self.terminal_pane_project(pane).is_some_and(|p| {
                monocode_core::files::path_key(&p) == monocode_core::files::path_key(project)
            })
        });
        let tree = preset_layout(preset, &panes).ok_or("ids must not be empty")?;
        self.workspace
            .arrange_panes(tree, reuse)
            .ok_or("The terminals could not be arranged")?;
        // Moving a pane clears its Logs identity, so it is set once they are in place.
        for pane in &panes {
            if let Some(state) = self.workspace.terminal_panes.get_mut(pane) {
                state.logs = Some(title.clone());
            }
        }
        self.pane_motion = None;
        self.history
            .prune(&self.workspace.open_sessions(), self.workspace.active);
        let revealed =
            !reveal || self.reveal_mono_terminal(project, Spot::Pane(panes[0]), switch, from, cx);
        self.fit_sidebar(cx);
        self.persist(cx);
        cx.notify();
        let mut done = json!({"tab": title, "ids": ids});
        if !revealed {
            done["revealed"] = json!(false);
        }
        Ok(done)
    }

    fn mono_send_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        approved: Option<&Request>,
        cx: &mut Context<Self>,
    ) -> Result<Step, String> {
        let id = id_of(input)?;
        let text = parse_typed(input, "text")?;
        let key = match input.get("key") {
            None => None,
            Some(name) => {
                let name = name
                    .as_str()
                    .ok_or_else(|| format!("key must be one of {}", MonoKey::NAMES))?;
                let key = MonoKey::parse(name)
                    .ok_or_else(|| format!("Unknown key {name}. Use one of {}", MonoKey::NAMES))?;
                Some((name, key))
            }
        };
        if text.is_none() && key.is_none() {
            return Err(format!("Provide text or key: one of {}", MonoKey::NAMES));
        }
        let found = self.find_own_terminal(mono, project, &id)?;
        // Everything below is asked of the terminal now: the poll behind `foreground` can be
        // a second old.
        let (label, seen) = {
            let t = found.terminal.read(cx);
            if t.exited {
                return Err("The terminal has exited".into());
            }
            // One probe: the front says both who holds the terminal and whether it is an agent.
            let seen = Seen::of(t);
            if seen.front.agent().is_some() {
                return Err("This terminal runs an agent CLI; Mono never types into those".into());
            }
            (t.display_title().to_owned(), seen)
        };
        if let Some((_, key)) = key
            && let Some(refusal) = key_refusal(key, &seen.front, text.is_some())
        {
            return Err(refusal.into());
        }
        // A key that recalls or completes a line waits for the user, whatever the program.
        let gated = key.is_some_and(|(_, key)| key.needs_approval());
        if let Some(text) = &text {
            if seen
                .cwd
                .as_deref()
                .is_some_and(|cwd| !live_cwd_inside(project, cwd))
            {
                return Err(OUTSIDE.into());
            }
            if let Some(refusal) = multiline_refusal(text, seen.bracketed) {
                return Err(refusal.into());
            }
        }
        if text.is_some() || gated {
            let multiline = text.as_deref().is_some_and(|t| t.contains('\n'));
            match approved {
                Some(request) => {
                    let same = request
                        .seen
                        .as_ref()
                        .is_some_and(|shown| shown.still(&seen, multiline));
                    if !same {
                        return Err(CHANGED.into());
                    }
                }
                None => {
                    let listed = text
                        .as_deref()
                        .is_none_or(|text| self.mono_allows(mono, project, "text", text, cx));
                    if !listed || gated {
                        let t = found.terminal.read(cx);
                        let started = t.cwd.clone();
                        let screen = gated.then(|| screen_tail(&t.scrollback_text()));
                        return Ok(Step::Ask(Box::new(Request {
                            kind: if text.is_some() { "text" } else { "key" },
                            text: text.clone().unwrap_or_default(),
                            label,
                            cwd_live: seen.cwd.is_some(),
                            cwd: seen.cwd.clone().unwrap_or(started),
                            seen: Some(seen),
                            then_key: key.map(|(name, _)| name.to_owned()),
                            screen,
                            input: input.clone(),
                            action: "terminals.send",
                        })));
                    }
                }
            }
        }
        if let Some(text) = &text {
            found
                .terminal
                .update(cx, |t, cx| t.send_text(text, false, cx));
        }
        if let Some((_, key)) = key {
            found.terminal.update(cx, |t, cx| t.send_key(key, cx));
        }
        Ok(Step::Done(match (&text, key) {
            (None, Some((name, _))) => json!({"sent": name}),
            (Some(text), key) => {
                json!({"sent": "text", "bytes": text.len(), "key": key.map(|(name, _)| name)})
            }
            (None, None) => unreachable!("checked above"),
        }))
    }

    fn mono_read_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let id = id_of(input)?;
        let lines = match input.get("lines") {
            None => READ_LINES_DEFAULT,
            Some(v) => match v.as_u64() {
                Some(0) | None => return Err("lines must be a positive integer".into()),
                Some(n) => (n as usize).min(READ_LINES_MAX),
            },
        };
        let found = self.find_mono_terminal(mono, project, &id)?;
        let t = found.terminal.read(cx);
        if found.origin == Origin::User && !t.mono_read {
            return Err(
                "The user has not allowed Mono to read this terminal (Let Mono read, in its tab menu)"
                    .into(),
            );
        }
        // Masked over the whole scrollback first, then cut: a secret that starts above the
        // window (a private key's first line) must not survive as an unmarked tail.
        let all = monocode_core::mono::memory::redact(&t.scrollback_text());
        let (text, older) = tail_lines(&all, lines);
        let (text, capped) = cap_tail(text, READ_BYTES_MAX);
        Ok(json!({
            "text": text,
            "truncated": older || capped,
            "running": t.foreground.is_some() && !t.exited,
            "exited": t.exited,
            "exitCode": t.exit_code,
        }))
    }

    fn mono_stop_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let found = self.find_own_terminal(mono, project, &id_of(input)?)?;
        // The job in front when stop was called. Only that group is hung up later: a job the
        // shell started in the meantime was not asked to stop.
        let stopping = found.terminal.read(cx).foreground_pgrp();
        found
            .terminal
            .update(cx, |t, cx| t.send_key(MonoKey::CtrlC, cx));
        if let Some(pgrp) = stopping {
            let weak = found.terminal.downgrade();
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(STOP_GRACE).await;
                let _ = weak.update(cx, |t, cx| t.hang_up_group(pgrp, cx));
            })
            .detach();
        }
        Ok(json!({"stopping": true}))
    }

    fn mono_close_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let found = self.find_own_terminal(mono, project, &id_of(input)?)?;
        found.terminal.read(cx).kill();
        match found.spot {
            Spot::Dock(ix) => self.close_mono_dock_terminal(project, ix, cx),
            Spot::Pane(pane) => self.close_mono_pane(pane, cx),
        }
        Ok(json!({"closed": true}))
    }

    /// The user's close of a dock tab, without the prompt and without moving the keyboard.
    fn close_mono_dock_terminal(&mut self, project: &str, ix: usize, cx: &mut Context<Self>) {
        // A drag or an open tab menu of the dock names tabs by position.
        self.cancel_dock_gestures(project, cx);
        if let Some(live) = self.terminals.get_mut(project)
            && ix < live.len()
        {
            live.remove(ix);
        }
        let current = monocode_core::files::path_key(project)
            == monocode_core::files::path_key(&self.workspace.project);
        let mut emptied = false;
        if let Some(dock) = self.workspace.docks.get_mut(project)
            && !dock.close(ix)
        {
            dock.open = false;
            emptied = true;
        }
        self.dock_shifts.clear();
        if emptied && current {
            self.slide_dock(false, cx);
            // The panel's Terminal tab is never left empty.
            if self.terminal_in_panel(cx) {
                self.ensure_panel_terminal(cx);
            }
            self.fit_sidebar(cx);
        }
        self.persist(cx);
        cx.notify();
    }

    /// A session that lends Mono terminal panes their scope is going away: each Mono pane it
    /// owns moves to another local session of the same project, or to the project's dock when
    /// none is left, keeping its shell, label, Mono and last run. Call before the session's
    /// summary is dropped (the pane's project is the owner's).
    pub(super) fn rehome_mono_panes(&mut self, gone: i64, cx: &mut Context<Self>) {
        let panes: Vec<(i64, String)> = self
            .workspace
            .terminal_panes
            .iter()
            .filter(|(_, state)| state.owner == gone && state.mono.is_some())
            .map(|(pane, _)| {
                let project = self
                    .terminal_pane_project(*pane)
                    .unwrap_or_else(|| self.workspace.project.clone());
                (*pane, project)
            })
            .collect();
        if panes.is_empty() {
            return;
        }
        for (pane, project) in panes {
            match self.mono_pane_owner(&project, Some(gone)) {
                Some(owner) => {
                    if let Some(state) = self.workspace.terminal_panes.get_mut(&pane) {
                        state.owner = owner;
                    }
                }
                None => self.move_mono_pane_to_dock(pane, &project, cx),
            }
        }
        self.persist(cx);
        cx.notify();
    }

    /// A Mono terminal pane becomes a tab of its project's dock, keeping its live shell and
    /// what the Mono saved for it. The dock stays as the user had it.
    fn move_mono_pane_to_dock(&mut self, pane: i64, project: &str, cx: &mut Context<Self>) {
        let Some(state) = self.workspace.terminal_panes.get(&pane).cloned() else {
            return;
        };
        let Some(view) = self.terminal_pane_view(pane, cx) else {
            return;
        };
        let Some(mono) = state.mono.clone() else {
            return;
        };
        if self.workspace.tab_of(pane).is_some() {
            let before = self.active_leaves();
            let split = self
                .workspace
                .tab_of(pane)
                .is_some_and(|tab| self.workspace.layout(tab).is_split());
            if !(split && self.workspace.close_pane(pane)) {
                self.workspace.close(pane);
            }
            self.begin_pane_motion(before, cx);
        }
        self.workspace.terminal_panes.remove(&pane);
        self.terminal_pane_views.remove(&pane);
        self.ensure_terminals(project, cx);
        self.cancel_dock_gestures(project, cx);
        let label = state
            .label
            .clone()
            .or_else(|| view.read(cx).label.clone())
            .unwrap_or_default();
        let dock = self.workspace.docks.entry(project.to_owned()).or_default();
        let before = (!dock.terminals.is_empty()).then_some((dock.active, dock.open));
        let ix = dock.add(state.cwd.clone());
        dock.set_mono_meta(
            ix,
            Some(MonoTerminalMeta {
                mono,
                label,
                run: state.run.clone(),
            }),
        );
        match before {
            Some((active, open)) => {
                dock.active = active;
                dock.open = open;
            }
            None => dock.open = false,
        }
        self.terminals
            .entry(project.to_owned())
            .or_default()
            .push(view);
        self.dock_shifts.clear();
        self.history
            .prune(&self.workspace.open_sessions(), self.workspace.active);
        self.fit_sidebar(cx);
    }

    /// The user's close of a terminal pane, without the prompt and without moving the
    /// keyboard or leaving a Mono's view.
    fn close_mono_pane(&mut self, pane: i64, cx: &mut Context<Self>) {
        let before = self.active_leaves();
        let split = self
            .workspace
            .tab_of(pane)
            .is_some_and(|tab| self.workspace.layout(tab).is_split());
        if !(split && self.workspace.close_pane(pane)) {
            self.workspace.close(pane);
        }
        self.history
            .prune(&self.workspace.open_sessions(), self.workspace.active);
        self.navigation_revision += 1;
        self.begin_pane_motion(before, cx);
        self.fit_sidebar(cx);
        // Prunes the pane's state and drops its shell.
        self.persist(cx);
        cx.notify();
    }

    /// The "Let Mono read" switch of the terminal with entity id `entity`: the id the menu was
    /// opened for, not a tab position or pane that may mean another terminal by the time the
    /// item is chosen. Does nothing when that terminal is gone.
    pub(super) fn toggle_mono_read(&mut self, entity: u64, cx: &mut Context<Self>) {
        let view = self
            .terminals
            .values()
            .flatten()
            .chain(self.terminal_pane_views.values())
            .find(|t| t.entity_id().as_u64() == entity)
            .cloned();
        if let Some(view) = view {
            view.update(cx, |t, cx| {
                t.mono_read = !t.mono_read;
                cx.notify();
            });
        }
    }

    /// Whether "Let Mono read" means anything for `terminal`: a plain shell the user opened.
    /// A Mono's own terminal is readable already, and a launched program (agent CLI, login,
    /// cloud shell) is never read.
    pub(super) fn offers_mono_read(terminal: &Terminal) -> bool {
        terminal.is_plain_shell() && !terminal.by_mono
    }

    /// The menu item that switches "Let Mono read" for `terminal`. Its id carries the terminal.
    pub(super) fn mono_read_item(terminal: &Entity<Terminal>, on: bool) -> MenuItem {
        MenuAction::new(
            format!("mono-read:{}", terminal.entity_id().as_u64()),
            if on {
                "Stop letting Mono read"
            } else {
                "Let Mono read"
            },
        )
        .into()
    }

    /// The terminal id inside a `mono-read:<id>` menu action.
    pub(super) fn mono_read_target(action: &str) -> Option<u64> {
        action.strip_prefix("mono-read:")?.parse().ok()
    }
}

/// The title of a Mono's Logs tab for a project.
fn logs_title(project: &str) -> String {
    format!("Logs {}", crate::workspace::project_name(project))
}

/// The small muted mark on the tab or header of a terminal a Mono opened.
pub(super) fn mono_mark(
    id: impl Into<gpui::ElementId>,
    selector: String,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .role(Role::Image)
        .aria_label("Opened by Mono")
        .flex_shrink_0()
        .flex()
        .items_center()
        .child(icon("bot", 12.).text_color(content(0.4)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{shell::tests::window, workspace::ProjectAction};
    use gpui::{TestAppContext, WindowHandle};

    const PROJECT: &str = "/work/monocode";

    fn call(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        mono: &str,
        action: &str,
        input: Value,
    ) -> Result<Value, String> {
        match ask(window, cx, mono, action, input) {
            MonoTerminalReply::Now(result) => result,
            MonoTerminalReply::Later(_) => panic!("{action} waited for the user"),
        }
    }

    fn now() -> Via {
        Via {
            session: 0,
            received: Instant::now(),
        }
    }

    /// The call as the host makes it: with a window, then the dialog is put in front.
    fn ask(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        mono: &str,
        action: &str,
        input: Value,
    ) -> MonoTerminalReply {
        ask_via(window, cx, mono, action, input, now())
    }

    fn ask_via(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        mono: &str,
        action: &str,
        input: Value,
        via: Via,
    ) -> MonoTerminalReply {
        window
            .update(cx, |shell, w, cx| {
                let reply = shell.mono_terminals_call(mono, PROJECT, action, &input, via, cx);
                shell.show_mono_approval(w, cx);
                reply
            })
            .unwrap()
    }

    /// A call that must wait for the user: its channel, still empty.
    fn wait(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        mono: &str,
        action: &str,
        input: Value,
    ) -> async_channel::Receiver<Result<Value, String>> {
        match ask(window, cx, mono, action, input) {
            MonoTerminalReply::Later(answer) => {
                assert!(answer.try_recv().is_err(), "answered before the user chose");
                answer
            }
            MonoTerminalReply::Now(result) => panic!("{action} did not wait: {result:?}"),
        }
    }

    /// The Monos the tests speak for, with the project, as the roster has them.
    fn add_monos(window: WindowHandle<Shell>, cx: &mut TestAppContext, ids: &[&str]) {
        window
            .update(cx, |shell, _, cx| {
                shell.mono_manager().update(cx, |monos, _| {
                    for id in ids {
                        monos.roster.monos.push(monocode_core::mono::Mono {
                            id: (*id).into(),
                            name: format!("Mono {id}"),
                            projects: vec![PROJECT.into()],
                            ..Default::default()
                        });
                    }
                })
            })
            .unwrap();
    }

    fn choose(window: WindowHandle<Shell>, cx: &mut TestAppContext, choice: mono_approval::Choice) {
        window
            .update(cx, |shell, w, cx| {
                let seq = shell.mono_approvals.queue.front().expect("a dialog").seq;
                shell.answer_mono_approval(seq, choice, w, cx)
            })
            .unwrap();
    }

    fn waiting(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> usize {
        window
            .update(cx, |shell, _, _| shell.mono_approvals.queue.len())
            .unwrap()
    }

    fn all_terminals(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> usize {
        window
            .update(cx, |shell, _, _| {
                shell.terminals.values().map(Vec::len).sum::<usize>()
            })
            .unwrap()
    }

    fn open(window: WindowHandle<Shell>, cx: &mut TestAppContext, input: Value) -> String {
        call(window, cx, "m1", "terminals.open", input).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn dock(window: WindowHandle<Shell>, cx: &mut TestAppContext, label: &str) -> String {
        open(window, cx, json!({"label": label, "place": "dock"}))
    }

    fn dock_as(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        mono: &str,
        label: &str,
    ) -> String {
        call(
            window,
            cx,
            mono,
            "terminals.open",
            json!({"label": label, "place": "dock"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn terminal(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        id: &str,
    ) -> Entity<Terminal> {
        window
            .update(cx, |shell, _, _| {
                shell
                    .find_mono_terminal("m1", PROJECT, id)
                    .unwrap()
                    .terminal
            })
            .unwrap()
    }

    fn sent(window: WindowHandle<Shell>, cx: &mut TestAppContext, id: &str) -> Vec<u8> {
        let t = terminal(window, cx, id);
        window
            .update(cx, |_, _, cx| t.read(cx).sent_bytes())
            .unwrap()
    }

    /// A shell the user opened in the project's dock, as the dock's "new terminal" does.
    fn user_terminal(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> Entity<Terminal> {
        window
            .update(cx, |shell, _, cx| {
                let t = shell.spawn_terminal(PROJECT.into(), cx);
                shell
                    .terminals
                    .entry(PROJECT.into())
                    .or_default()
                    .push(t.clone());
                shell
                    .workspace
                    .docks
                    .entry(PROJECT.into())
                    .or_default()
                    .add(PROJECT.into());
                t
            })
            .unwrap()
    }

    fn rows(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> Vec<Value> {
        call(window, cx, "m1", "terminals.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn the_screen_is_its_last_lines_each_cut_to_its_end() {
        assert_eq!(screen_tail("a\nb\nc\nd\n\n"), "b\nc\nd");
        assert_eq!(screen_tail("only"), "only");
        let long = format!("{}end", "x".repeat(500));
        let shown = screen_tail(&long);
        assert!(
            shown.starts_with("...") && shown.ends_with("xxend"),
            "{shown}"
        );
        assert_eq!(shown.chars().count(), SCREEN_COLUMNS + 3);
    }

    #[gpui::test]
    fn ids_follow_the_terminal_through_dock_reorders_and_user_closes(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let (be, fe) = (dock(window, cx, "BE"), dock(window, cx, "FE"));
        let prefix = window
            .update(cx, |shell, _, _| shell.mono_registry.prefix.clone())
            .unwrap();
        let id = |n: u32| format!("t-{prefix}-{n}");
        assert_eq!((be.as_str(), fe.as_str()), (id(1).as_str(), id(2).as_str()));
        // The user drags FE in front of BE.
        window
            .update(cx, |shell, _, _| {
                shell
                    .workspace
                    .docks
                    .get_mut(PROJECT)
                    .unwrap()
                    .reorder(1, 0);
                shell.terminals.get_mut(PROJECT).unwrap().swap(0, 1);
            })
            .unwrap();
        call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": be, "key": "ctrl-c"}),
        )
        .unwrap();
        assert_eq!(sent(window, cx, &be), b"\x03");
        assert!(sent(window, cx, &fe).is_empty());
        // The user closes BE's tab (now second).
        window
            .update(cx, |shell, w, cx| shell.remove_terminal(PROJECT, 1, w, cx))
            .unwrap();
        let gone = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": be, "key": "ctrl-c"}),
        );
        assert!(
            gone.unwrap_err()
                .contains(&format!("Unknown terminal {}", id(1)))
        );
        let list = rows(window, cx);
        assert_eq!(list.len(), 1);
        assert_eq!(
            (&list[0]["id"], &list[0]["label"]),
            (&json!(id(2)), &json!("FE"))
        );
        // Ids are not handed out twice.
        assert_eq!(dock(window, cx, "BE"), id(3));
    }

    #[gpui::test]
    fn the_list_skips_launched_programs_other_projects_and_other_monos(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let mine = dock(window, cx, "BE");
        call(
            window,
            cx,
            "m2",
            "terminals.open",
            json!({"label": "theirs", "place": "dock"}),
        )
        .unwrap();
        let user = user_terminal(window, cx);
        window
            .update(cx, |shell, _, cx| {
                // A launched program (a TUI agent, a cloud shell, a login) lives in the dock too.
                let launched = cx.new(|cx| {
                    Terminal::launch(Path::new("/tmp"), "sh".into(), vec![], vec![], vec![], cx)
                });
                shell
                    .terminals
                    .entry(PROJECT.into())
                    .or_default()
                    .push(launched);
                let other = shell.spawn_terminal("/work/another-project".into(), cx);
                shell
                    .terminals
                    .entry("/work/another-project".into())
                    .or_default()
                    .push(other);
            })
            .unwrap();
        let list = rows(window, cx);
        let owners: Vec<_> = list
            .iter()
            .map(|r| (r["id"].as_str().unwrap(), r["owner"].as_str().unwrap()))
            .collect();
        assert_eq!(owners.len(), 2, "{list:?}");
        assert_eq!(owners[0], (mine.as_str(), "mono"));
        assert_eq!(owners[1].1, "user");
        let user_id = owners[1].0.to_owned();
        assert_eq!(terminal(window, cx, &user_id), user);
        assert_eq!(list[0]["place"], "dock");
        assert_eq!(list[0]["running"], false);
    }

    #[gpui::test]
    fn an_idle_terminal_with_the_same_label_is_reused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let first = dock(window, cx, "BE");
        let again = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(
            (again["id"].as_str(), again["reused"].as_bool()),
            (Some(first.as_str()), Some(true))
        );
        assert_eq!(rows(window, cx).len(), 1);
        // Busy: a second terminal of the same name opens.
        let t = terminal(window, cx, &first);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.foreground = Some("sleep".into()))
            })
            .unwrap();
        let busy = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(busy["reused"], false);
        assert_ne!(busy["id"].as_str(), Some(first.as_str()));
        // Another Mono's terminal of that name is not reused.
        let other = call(
            window,
            cx,
            "m2",
            "terminals.open",
            json!({"label": "BE", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(other["reused"], false);
    }

    #[gpui::test]
    fn a_saved_action_runs_in_the_new_or_reused_terminal(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, _, _| {
                shell.workspace.actions.insert(
                    PROJECT.into(),
                    vec![ProjectAction {
                        id: "a1".into(),
                        name: "Dev server".into(),
                        command: "npm run dev".into(),
                        ..Default::default()
                    }],
                );
            })
            .unwrap();
        let unknown = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "action": "nope"}),
        );
        assert!(unknown.unwrap_err().contains("a1 (Dev server)"));
        assert!(rows(window, cx).is_empty());
        let id = open(
            window,
            cx,
            json!({"label": "BE", "action": "a1", "place": "dock"}),
        );
        assert_eq!(sent(window, cx, &id), b"npm run dev\r");
        // Idle again: the same terminal runs it once more.
        let t = terminal(window, cx, &id);
        window
            .update(cx, |_, _, cx| t.update(cx, |t, _| t.foreground = None))
            .unwrap();
        let again = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "action": "a1", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(again["reused"], true);
        assert_eq!(sent(window, cx, &id), b"npm run dev\rnpm run dev\r");
    }

    #[gpui::test]
    fn labels_are_checked_and_shown_instead_of_the_title(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        for bad in [
            json!({}),
            json!({"label": "  "}),
            json!({"label": "x".repeat(41)}),
            json!({"label": "a\nb"}),
        ] {
            assert!(call(window, cx, "m1", "terminals.open", bad).is_err());
        }
        let ok = open(
            window,
            cx,
            json!({"label": "x".repeat(40), "place": "dock"}),
        );
        let t = terminal(window, cx, &ok);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.title = "zsh".into();
                    assert_eq!(t.display_title(), "x".repeat(40));
                })
            })
            .unwrap();
    }

    #[gpui::test]
    fn split_terminals_share_one_tab_beside_the_chat(cx: &mut TestAppContext) {
        let (window, _, chat) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.workspace.preferences.set_terminal_placement("title");
                shell.open(chat, w, cx);
            })
            .unwrap();
        // No place: the title placement means a pane.
        let be = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "reveal": false}),
        )
        .unwrap();
        // Alone in a Logs tab of its own: the user's chat tab is never split by a Mono.
        assert_eq!(be["place"], "tab");
        let fe = open(
            window,
            cx,
            json!({"label": "FE", "beside": be["id"], "direction": "down", "reveal": false}),
        );
        window
            .update(cx, |shell, _, cx| {
                let tab = shell.workspace.tab_of(chat).unwrap();
                assert_eq!(shell.workspace.layout(tab).leaf_ids(), vec![chat]);
                assert_eq!(shell.workspace.terminal_panes.len(), 2);
                let logs = shell
                    .workspace
                    .tab_of(*shell.workspace.terminal_panes.keys().next().unwrap())
                    .unwrap();
                assert_ne!(logs, tab);
                assert_eq!(shell.workspace.layout(logs).leaf_ids().len(), 2);
                assert_eq!(shell.workspace.logs_title(logs), Some("Logs monocode"));
                for pane in shell.workspace.terminal_panes.values() {
                    assert_eq!(pane.owner, chat);
                    assert_eq!(pane.mono.as_deref(), Some("m1"));
                }
                // Opening is not navigating: the chat is still the active pane.
                assert_eq!(shell.workspace.active, Some(chat));
                let titles: Vec<_> = shell
                    .workspace
                    .terminal_panes
                    .keys()
                    .filter_map(|p| shell.terminal_pane_title(*p, cx))
                    .collect();
                assert!(titles.contains(&"BE".to_owned()) && titles.contains(&"FE".to_owned()));
            })
            .unwrap();
        let list = rows(window, cx);
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|r| r["place"] == "split"));
        // A terminal in the dock has no pane to split beside.
        let in_dock = dock(window, cx, "DB");
        let err = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "X", "place": "split", "beside": in_dock}),
        );
        assert!(err.unwrap_err().contains("dock"));
        let _ = fe;
    }

    #[gpui::test]
    fn a_split_with_nothing_to_split_beside_gets_its_own_tab(cx: &mut TestAppContext) {
        let (window, _, chat) = window(cx);
        window
            .update(cx, |shell, _, _| {
                // No active pane at all.
                shell.workspace.active = None;
                shell.workspace.project_focus.insert(PROJECT.into(), chat);
            })
            .unwrap();
        let before = window.update(cx, |s, _, _| s.workspace.tabs.len()).unwrap();
        let opened = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "split", "reveal": false}),
        )
        .unwrap();
        assert_eq!(opened["place"], "tab");
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.tabs.len(), before + 1);
                assert_eq!(shell.workspace.active, None);
            })
            .unwrap();
    }

    #[gpui::test]
    fn reveal_shows_a_terminal_without_taking_the_keyboard_or_navigating(cx: &mut TestAppContext) {
        let (window, _, chat) = window(cx);
        window
            .update(cx, |shell, w, cx| shell.open(chat, w, cx))
            .unwrap();
        let snapshot = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, w, cx| {
                    (
                        w.focused(cx),
                        shell.navigation_revision,
                        shell.workspace.project.clone(),
                    )
                })
                .unwrap()
        };
        let before = snapshot(cx);
        // Dock: hidden until revealed.
        let hidden = open(
            window,
            cx,
            json!({"label": "BE", "place": "dock", "reveal": false}),
        );
        assert!(
            !window
                .update(cx, |s, _, _| s.workspace.docks[PROJECT].open)
                .unwrap()
        );
        assert_eq!(snapshot(cx), before);
        call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "dock", "reveal": true}),
        )
        .unwrap();
        assert!(
            window
                .update(cx, |s, _, _| s.workspace.docks[PROJECT].open)
                .unwrap()
        );
        assert_eq!(snapshot(cx), before);
        // Pane, default reveal: the Mono's new tab joins the strip, but the user stays where
        // they were, with the keyboard where it was.
        window
            .update(cx, |shell, _, _| {
                shell.workspace.preferences.set_terminal_placement("title")
            })
            .unwrap();
        let (tabs, active) = window
            .update(cx, |s, _, _| (s.workspace.tabs.len(), s.workspace.active))
            .unwrap();
        let quiet = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "FE", "place": "split"}),
        )
        .unwrap();
        assert_eq!(quiet["revealed"], false, "{quiet}");
        assert_eq!(snapshot(cx), before);
        window
            .update(cx, |s, _, _| {
                assert_eq!(s.workspace.tabs.len(), tabs + 1);
                assert_eq!(s.workspace.active, active);
            })
            .unwrap();
        // An explicit reveal may switch tabs, and then the keyboard is not given to the
        // terminal: the composer that had it is hidden now, so the shell root takes it.
        let shown = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE2", "place": "split", "reveal": true}),
        )
        .unwrap();
        assert!(shown.get("revealed").is_none(), "{shown}");
        let pane = shown["id"].as_str().unwrap().to_owned();
        window
            .update(cx, |shell, w, cx| {
                let Spot::Pane(p) = shell.find_mono_terminal("m1", PROJECT, &pane).unwrap().spot
                else {
                    panic!("not a pane");
                };
                assert_eq!(shell.workspace.active, Some(p));
                let terminal_focus = shell.terminal_pane_views[&p].focus_handle(cx);
                assert!(w.focused(cx) != Some(terminal_focus));
                assert_eq!(w.focused(cx), Some(shell.focus.clone()));
                assert_eq!(shell.navigation_revision, before.1);
                assert_eq!(shell.workspace.project, before.2);
            })
            .unwrap();
        let _ = hidden;
    }

    #[gpui::test]
    fn a_default_reveal_shows_a_pane_only_in_the_tab_the_user_is_looking_at(
        cx: &mut TestAppContext,
    ) {
        let (window, _chat) = logs_setup(cx);
        // The first open makes the Logs tab in the strip without selecting it.
        let first = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "split"}),
        )
        .unwrap();
        assert_eq!(first["revealed"], false);
        let active = window.update(cx, |s, _, _| s.workspace.active).unwrap();
        // The user opens that tab; now a new split lands in the visible tab and is revealed.
        let be = first["id"].as_str().unwrap().to_owned();
        window
            .update(cx, |shell, _, cx| {
                let Spot::Pane(p) = shell.find_mono_terminal("m1", PROJECT, &be).unwrap().spot
                else {
                    panic!("not a pane");
                };
                shell.reveal_terminal_pane(p, cx);
            })
            .unwrap();
        let second = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "FE", "place": "split"}),
        )
        .unwrap();
        assert!(second.get("revealed").is_none(), "{second}");
        assert_ne!(
            window.update(cx, |s, _, _| s.workspace.active).unwrap(),
            active
        );
    }

    #[test]
    fn a_folder_is_compared_through_its_symlinks() {
        #[cfg(unix)]
        {
            let dir = std::env::temp_dir().join(format!("mono-real-{}", std::process::id()));
            let real = dir.join("real");
            std::fs::create_dir_all(&real).unwrap();
            let link = dir.join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let (real, link) = (real.to_string_lossy(), link.to_string_lossy());
            assert_ne!(
                monocode_core::files::path_key(&real),
                monocode_core::files::path_key(&link)
            );
            assert_eq!(real_key(&real), real_key(&link));
            let _ = std::fs::remove_dir_all(&dir);
        }
        // A folder that does not exist is compared as spelled.
        assert_eq!(
            real_key("/no/such/dir/"),
            monocode_core::files::path_key("/no/such/dir/")
        );
    }

    #[test]
    fn an_unknown_front_is_reused_by_the_poll_only_where_the_poll_can_see() {
        let (shell, program) = (Front::Shell("zsh".into()), Front::Program("vite".into()));
        for unix in [true, false] {
            assert!(idle_for_reuse(&shell, false, unix));
            assert!(!idle_for_reuse(&program, true, unix));
        }
        // Unix: no program at the last poll means the same idle terminal.
        assert!(idle_for_reuse(&Front::Unknown, true, true));
        assert!(!idle_for_reuse(&Front::Unknown, false, true));
        // Elsewhere the poll always says "nothing", which proves nothing: never reused.
        assert!(!idle_for_reuse(&Front::Unknown, true, false));
        assert!(!idle_for_reuse(&Front::Unknown, false, false));
    }

    #[gpui::test]
    fn an_unknown_foreground_reuses_by_the_poll_and_asks_for_every_history_key(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let first = dock(window, cx, "BE");
        let t = terminal(window, cx, &first);
        // `ps` failed or this is Windows: the live answer is "unknown".
        set_front(window, cx, &t, Some(Front::Unknown));
        let again = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "dock"}),
        )
        .unwrap();
        // Off unix the poll proves nothing, so a program could be running: a new terminal
        // opens under the same label instead of reusing this one.
        assert_eq!(again["reused"].as_bool(), Some(cfg!(unix)), "{again}");
        assert_eq!(rows(window, cx).len(), if cfg!(unix) { 1 } else { 2 });
        assert_eq!(Front::Unknown.name(), "unknown");
        // Enter is never free there; the history keys ask.
        assert!(send_key(window, cx, &first, "enter").is_err());
        for key in ["up", "down", "tab", "esc"] {
            let answer = press(window, cx, &first, key);
            choose(window, cx, mono_approval::Choice::Deny);
            assert!(answer.try_recv().unwrap().is_err(), "{key}");
        }
        assert!(sent(window, cx, &first).is_empty());
        // The last poll saw a program: not the same idle terminal, so a new one opens.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.foreground = Some("sleep".into()))
            })
            .unwrap();
        let busy = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(busy["reused"], false);
    }

    #[gpui::test]
    fn beside_must_be_one_of_the_monos_own_terminals(cx: &mut TestAppContext) {
        let (window, _chat) = logs_setup(cx);
        let user = user_terminal(window, cx);
        let user_id = rows(window, cx)
            .iter()
            .find(|r| r["owner"] == "user")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let _ = user;
        let err = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "X", "place": "split", "beside": user_id}),
        )
        .unwrap_err();
        assert!(err.contains("this Mono opened"), "{err}");
        // Another Mono's terminal is not its own either.
        let other = dock_as(window, cx, "m2", "OTHER");
        let err = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "X", "place": "split", "beside": other}),
        )
        .unwrap_err();
        assert!(
            err.contains("Unknown terminal") || err.contains("this Mono opened"),
            "{err}"
        );
        assert_eq!(rows(window, cx).len(), 1, "nothing opened");
    }

    fn set_front(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        t: &Entity<Terminal>,
        front: Option<Front>,
    ) {
        window
            .update(cx, |_, _, cx| t.update(cx, |t, _| t.fake.front = front))
            .unwrap();
    }

    fn program(name: &str) -> Option<Front> {
        Some(Front::Program(name.into()))
    }

    fn send_key(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        id: &str,
        key: &str,
    ) -> Result<Value, String> {
        call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "key": key}),
        )
    }

    /// A key press that must wait for the user: its channel, still empty.
    fn press(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        id: &str,
        key: &str,
    ) -> async_channel::Receiver<Result<Value, String>> {
        wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "key": key}),
        )
    }

    /// The dialog in front: its kind, key, program and screen as shown.
    fn front_dialog(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
    ) -> (&'static str, Option<String>, String, Option<String>) {
        window
            .update(cx, |shell, _, _| {
                let request = &shell
                    .mono_approvals
                    .queue
                    .front()
                    .expect("a dialog")
                    .request;
                (
                    request.kind,
                    request.then_key.clone(),
                    request.seen.as_ref().unwrap().front.name().to_owned(),
                    request.screen.clone(),
                )
            })
            .unwrap()
    }

    #[gpui::test]
    fn only_the_known_keys_reach_the_terminal(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        // A program holds the terminal. Ctrl-C, enter and q are free for it.
        set_front(window, cx, &t, program("vite"));
        let mut want = Vec::new();
        for (key, bytes) in [("ctrl-c", &b"\x03"[..]), ("enter", b"\r"), ("q", b"q")] {
            send_key(window, cx, &id, key).unwrap();
            want.extend_from_slice(bytes);
            assert_eq!(sent(window, cx, &id), want, "{key}");
        }
        // Up, down, tab and esc wait for the user even in front of a plain program, and Run
        // sends exactly their bytes.
        for (key, bytes) in [
            ("up", &b"\x1b[A"[..]),
            ("down", b"\x1b[B"),
            ("tab", b"\t"),
            ("esc", b"\x1b"),
        ] {
            let answer = press(window, cx, &id, key);
            assert_eq!(sent(window, cx, &id), want, "{key} waited");
            choose(window, cx, mono_approval::Choice::Run);
            assert_eq!(answer.try_recv().unwrap().unwrap()["sent"], key);
            want.extend_from_slice(bytes);
            assert_eq!(sent(window, cx, &id), want, "{key}");
        }
        for bad in [
            json!({"id": id, "key": "ctrl-d"}),
            json!({"id": id}),
            json!({"key": "q"}),
        ] {
            assert!(call(window, cx, "m1", "terminals.send", bad).is_err());
        }
        assert_eq!(sent(window, cx, &id), want);
    }

    #[gpui::test]
    fn a_history_key_asks_with_the_screen_and_never_offers_allow(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        set_front(window, cx, &t, program("vite"));
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| {
                    t.fake_output("one\r\ntwo\r\n$ rm -rf build\r\n", cx)
                })
            })
            .unwrap();
        let answer = press(window, cx, &id, "up");
        let (kind, key, front, screen) = front_dialog(window, cx);
        assert_eq!(
            (kind, key.as_deref(), front.as_str()),
            ("key", Some("up"), "vite")
        );
        let screen = screen.expect("the screen is shown");
        assert!(screen.contains("$ rm -rf build"), "{screen}");
        // Allow is not offered, and answering Allow would not remember anything.
        window
            .update(cx, |shell, _, _| {
                let request = &shell.mono_approvals.queue.front().unwrap().request;
                assert!(!request.allowable());
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Allow);
        assert!(answer.try_recv().unwrap().is_ok());
        assert_eq!(sent(window, cx, &id), b"\x1b[A");
        // Asked again: nothing was remembered for a key.
        let again = press(window, cx, &id, "up");
        choose(window, cx, mono_approval::Choice::Deny);
        assert!(again.try_recv().unwrap().is_err());
        assert_eq!(sent(window, cx, &id), b"\x1b[A");
        // Another key of the same Mono waits behind the one rule: one request at a time.
        let first = press(window, cx, &id, "tab");
        let busy = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "key": "esc"}),
        )
        .unwrap_err();
        assert_eq!(busy, mono_approval::BUSY);
        choose(window, cx, mono_approval::Choice::Deny);
        assert!(first.try_recv().unwrap().is_err());
        assert_eq!(sent(window, cx, &id), b"\x1b[A");
    }

    #[gpui::test]
    fn a_key_approval_is_checked_again_when_it_is_written(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        set_front(window, cx, &t, program("vite"));
        let answer = press(window, cx, &id, "up");
        // The program in front changed after the dialog was shown.
        set_front(window, cx, &t, Some(Front::Shell("zsh".into())));
        choose(window, cx, mono_approval::Choice::Run);
        let err = answer.try_recv().unwrap().unwrap_err();
        assert!(err.contains("changed after the dialog"), "{err}");
        assert!(sent(window, cx, &id).is_empty());
    }

    #[gpui::test]
    fn keys_that_recall_or_run_a_history_line_wait_or_are_refused_at_a_shell_prompt(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        // The shell itself holds the terminal: enter would run a recalled line with nobody
        // asked, so it is refused; up, down, tab and esc (history, completion, meta-key
        // recalls) ask the user instead.
        let err = send_key(window, cx, &id, "enter").unwrap_err();
        assert!(err.contains("shell"), "{err}");
        for key in ["up", "down", "tab", "esc"] {
            let answer = press(window, cx, &id, key);
            choose(window, cx, mono_approval::Choice::Deny);
            assert!(answer.try_recv().unwrap().is_err(), "{key}");
        }
        assert!(sent(window, cx, &id).is_empty());
        // Ctrl-C and q are harmless there.
        for key in ["ctrl-c", "q"] {
            send_key(window, cx, &id, key).unwrap();
        }
        assert_eq!(sent(window, cx, &id), b"\x03q");
        // Text approved in the same call may press Enter.
        let ok = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls", "key": "enter"}),
        );
        choose(window, cx, mono_approval::Choice::Run);
        assert!(ok.try_recv().unwrap().is_ok());
        assert_eq!(sent(window, cx, &id), b"\x03qls\r");
        // Text with a history key is one dialog that covers both, with no Allow, and the
        // key goes after the text.
        let both = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls", "key": "up"}),
        );
        assert_eq!(waiting(window, cx), 1);
        let (kind, key, _, screen) = front_dialog(window, cx);
        assert_eq!((kind, key.as_deref()), ("text", Some("up")));
        assert!(screen.is_some());
        window
            .update(cx, |shell, _, _| {
                assert!(!shell.mono_approvals.queue[0].request.allowable());
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Allow);
        assert!(both.try_recv().unwrap().is_ok());
        assert_eq!(sent(window, cx, &id), b"\x03qls\rls\x1b[A");
        // The text was not remembered by that Allow: it asks again.
        let again = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls", "key": "enter"}),
        );
        choose(window, cx, mono_approval::Choice::Deny);
        assert!(again.try_recv().unwrap().is_err());
    }

    #[gpui::test]
    fn remembered_text_still_asks_when_the_call_presses_a_history_key(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let remembered = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        choose(window, cx, mono_approval::Choice::Allow);
        assert!(remembered.try_recv().unwrap().is_ok());
        // The same text now runs without a dialog, but not with a key that recalls a line.
        call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls", "key": "enter"}),
        )
        .unwrap();
        let asked = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls", "key": "up"}),
        );
        choose(window, cx, mono_approval::Choice::Deny);
        assert!(asked.try_recv().unwrap().is_err());
        assert_eq!(sent(window, cx, &id), b"lsls\r");
    }

    #[gpui::test]
    fn a_program_that_is_a_shell_or_a_repl_gets_the_shell_key_rules(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        for name in ["ssh", "python3.12", "sudo", "tmux", "psql", "-bash"] {
            set_front(window, cx, &t, program(name));
            // Enter would run a recalled line: refused without approved text.
            let err = send_key(window, cx, &id, "enter").unwrap_err();
            assert!(err.contains("shell"), "{name}: {err}");
            // The history keys wait for the user, Deny sends nothing.
            for key in ["up", "down", "tab", "esc"] {
                let answer = press(window, cx, &id, key);
                choose(window, cx, mono_approval::Choice::Deny);
                assert!(answer.try_recv().unwrap().is_err(), "{name} {key}");
            }
            // Ctrl-C and q stay free.
            send_key(window, cx, &id, "ctrl-c").unwrap();
            send_key(window, cx, &id, "q").unwrap();
        }
        assert_eq!(sent(window, cx, &id), b"\x03q".repeat(6));
        // A plain program gets enter free.
        set_front(window, cx, &t, program("vim"));
        send_key(window, cx, &id, "enter").unwrap();
    }

    #[gpui::test]
    fn the_foreground_is_asked_live_and_not_taken_from_the_poll(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        // The poll still says "sleep" (a program), but the shell has the terminal again.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.foreground = Some("sleep".into());
                    t.fake.front = Some(Front::Shell("zsh".into()));
                })
            })
            .unwrap();
        assert!(send_key(window, cx, &id, "enter").is_err());
        let shell_up = press(window, cx, &id, "up");
        assert_eq!(front_dialog(window, cx).2, "zsh");
        choose(window, cx, mono_approval::Choice::Deny);
        assert!(shell_up.try_recv().unwrap().is_err());
        // The poll says nothing, but a program holds it now: enter is free.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.foreground = None;
                    t.fake.front = program("vite");
                })
            })
            .unwrap();
        send_key(window, cx, &id, "enter").unwrap();
        let live = press(window, cx, &id, "up");
        assert_eq!(front_dialog(window, cx).2, "vite");
        choose(window, cx, mono_approval::Choice::Run);
        assert!(live.try_recv().unwrap().is_ok());
        // An agent CLI that started since the last poll still gets nothing.
        set_front(window, cx, &t, program("claude"));
        let err = send_key(window, cx, &id, "q").unwrap_err();
        assert!(err.contains("agent CLI"), "{err}");
        assert_eq!(sent(window, cx, &id), b"\r\x1b[A");
        // A terminal that cannot be asked is not trusted as a program.
        set_front(window, cx, &t, Some(Front::Unknown));
        assert!(send_key(window, cx, &id, "enter").is_err());
    }

    #[gpui::test]
    fn a_terminal_running_an_agent_cli_gets_no_keys(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.foreground = Some("claude".into()))
            })
            .unwrap();
        let err = send_key(window, cx, &id, "enter");
        assert!(err.unwrap_err().contains("agent CLI"));
        assert!(sent(window, cx, &id).is_empty());
    }

    #[gpui::test]
    fn read_returns_the_tail_masked_and_capped(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        let secret = format!("ghp_{}", "a".repeat(30));
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| {
                    for n in 0..300 {
                        t.fake_output(&format!("line {n}\r\n"), cx);
                    }
                    t.fake_output(&format!("token {secret}\r\n"), cx);
                })
            })
            .unwrap();
        let read = call(window, cx, "m1", "terminals.read", json!({"id": id})).unwrap();
        let text = read["text"].as_str().unwrap();
        assert_eq!(read["truncated"], true);
        assert_eq!(text.lines().count(), 200);
        assert!(text.contains("line 299") && !text.contains("line 10\n"));
        assert!(!text.contains(&secret) && text.contains("redacted"));
        let two = call(
            window,
            cx,
            "m1",
            "terminals.read",
            json!({"id": id, "lines": 2}),
        )
        .unwrap();
        assert_eq!(two["text"].as_str().unwrap().lines().count(), 2);
        for bad in [
            json!({"id": id, "lines": 0}),
            json!({"id": id, "lines": "x"}),
        ] {
            assert!(call(window, cx, "m1", "terminals.read", bad).is_err());
        }
        // More than 1000 lines are asked for: 1000 at most, within 32 KiB.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| {
                    for n in 0..1500 {
                        t.fake_output(&format!("{n:04} {}\r\n", "x".repeat(70)), cx);
                    }
                })
            })
            .unwrap();
        let big = call(
            window,
            cx,
            "m1",
            "terminals.read",
            json!({"id": id, "lines": 99999}),
        )
        .unwrap();
        assert_eq!(big["truncated"], true);
        assert!(big["text"].as_str().unwrap().len() <= READ_BYTES_MAX);
        assert!(big["text"].as_str().unwrap().contains("1499 "));
    }

    #[test]
    fn the_cap_keeps_the_end_on_a_character_boundary() {
        let (kept, cut) = cap_tail("héllo wörld".into(), 6);
        assert!(cut && "héllo wörld".ends_with(&kept) && kept.len() <= 6);
        assert_eq!(cap_tail("abc".into(), 3), ("abc".into(), false));
    }

    #[gpui::test]
    fn stop_sends_ctrl_c_and_hangs_up_after_the_grace_period(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.foreground = Some("sleep".into()))
            })
            .unwrap();
        let stopped = call(window, cx, "m1", "terminals.stop", json!({"id": id})).unwrap();
        assert_eq!(stopped, json!({"stopping": true}));
        assert_eq!(sent(window, cx, &id), b"\x03");
        cx.run_until_parked();
        assert!(
            window
                .update(cx, |_, _, cx| t.read(cx).foreground.is_some())
                .unwrap()
        );
        cx.executor()
            .advance_clock(STOP_GRACE + Duration::from_millis(100));
        cx.run_until_parked();
        assert!(
            window
                .update(cx, |_, _, cx| t.read(cx).foreground.is_none())
                .unwrap()
        );
    }

    #[gpui::test]
    fn the_delayed_hang_up_only_signals_the_job_that_was_in_front_when_stop_was_called(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        let hung = |cx: &mut TestAppContext| {
            window
                .update(cx, |_, _, cx| t.read(cx).fake.hung_up.clone())
                .unwrap()
        };
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.foreground = Some("sleep".into());
                    t.fake.pgrp = Some(500);
                })
            })
            .unwrap();
        call(window, cx, "m1", "terminals.stop", json!({"id": id})).unwrap();
        // The job ended and the user (or the shell) started another one meanwhile.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.foreground = Some("vim".into());
                    t.fake.pgrp = Some(600);
                })
            })
            .unwrap();
        cx.executor()
            .advance_clock(STOP_GRACE + Duration::from_millis(100));
        cx.run_until_parked();
        assert!(hung(cx).is_empty(), "the new job was not asked to stop");
        // The same job still in front is hung up.
        call(window, cx, "m1", "terminals.stop", json!({"id": id})).unwrap();
        cx.executor()
            .advance_clock(STOP_GRACE + Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(hung(cx), [600]);
        // Nothing but the shell in front: nothing to hang up later.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.foreground = None;
                    t.fake.pgrp = None;
                })
            })
            .unwrap();
        call(window, cx, "m1", "terminals.stop", json!({"id": id})).unwrap();
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.foreground = Some("late".into());
                    t.fake.pgrp = Some(700);
                })
            })
            .unwrap();
        cx.executor()
            .advance_clock(STOP_GRACE + Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(hung(cx), [600]);
    }

    #[gpui::test]
    fn close_removes_the_dock_tab_and_the_pane(cx: &mut TestAppContext) {
        let (window, _, chat) = window(cx);
        let dock_id = dock(window, cx, "BE");
        let weak = terminal(window, cx, &dock_id).downgrade();
        call(window, cx, "m1", "terminals.close", json!({"id": dock_id})).unwrap();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.terminals[PROJECT].is_empty());
                assert!(shell.workspace.docks[PROJECT].terminals.is_empty());
            })
            .unwrap();
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
        assert!(rows(window, cx).is_empty());

        window
            .update(cx, |shell, w, cx| {
                shell.workspace.preferences.set_terminal_placement("title");
                shell.open(chat, w, cx);
            })
            .unwrap();
        let pane_id = open(
            window,
            cx,
            json!({"label": "FE", "place": "split", "reveal": false}),
        );
        let weak = terminal(window, cx, &pane_id).downgrade();
        call(window, cx, "m1", "terminals.close", json!({"id": pane_id})).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.workspace.terminal_panes.is_empty());
                assert!(shell.terminal_pane_views.is_empty());
                assert!(shell.workspace.tab_of(chat).is_some());
            })
            .unwrap();
        assert!(weak.upgrade().is_none());
    }

    #[gpui::test]
    fn user_terminals_cannot_be_driven_and_read_only_with_the_toggle(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let user = user_terminal(window, cx);
        let id = rows(window, cx)[0]["id"].as_str().unwrap().to_owned();
        assert_eq!(rows(window, cx)[0]["owner"], "user");
        for (action, input) in [
            ("terminals.send", json!({"id": id, "key": "enter"})),
            ("terminals.stop", json!({"id": id})),
            ("terminals.close", json!({"id": id})),
        ] {
            let err = call(window, cx, "m1", action, input).unwrap_err();
            assert!(err.contains("user opened"), "{action}: {err}");
        }
        assert!(
            window
                .update(cx, |_, _, cx| user.read(cx).sent_bytes().is_empty())
                .unwrap()
        );
        let refused = call(window, cx, "m1", "terminals.read", json!({"id": id})).unwrap_err();
        assert!(refused.contains("Let Mono read"));
        // The tab menu's switch names the terminal it was opened for.
        let toggle = format!("mono-read:{}", user.entity_id().as_u64());
        window
            .update(cx, |shell, w, cx| {
                shell.run_terminal_tab_menu(0, &toggle, w, cx)
            })
            .unwrap();
        assert!(call(window, cx, "m1", "terminals.read", json!({"id": id})).is_ok());
        window
            .update(cx, |shell, w, cx| {
                shell.run_terminal_tab_menu(0, &toggle, w, cx)
            })
            .unwrap();
        assert!(call(window, cx, "m1", "terminals.read", json!({"id": id})).is_err());
        // Another Mono's terminal is unknown, not merely read-only.
        let theirs = call(
            window,
            cx,
            "m2",
            "terminals.open",
            json!({"label": "T", "place": "dock"}),
        )
        .unwrap();
        let err = call(
            window,
            cx,
            "m1",
            "terminals.read",
            json!({"id": theirs["id"]}),
        )
        .unwrap_err();
        assert!(err.contains("Unknown terminal"));
    }

    #[gpui::test]
    fn remote_projects_and_unknown_ids_are_refused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let remote = window
            .update(cx, |shell, _, cx| {
                shell.mono_terminals_call(
                    "m1",
                    "remote://env/path",
                    "terminals.list",
                    &json!({}),
                    now(),
                    cx,
                )
            })
            .unwrap();
        let MonoTerminalReply::Now(remote) = remote else {
            panic!("a refusal is immediate");
        };
        assert!(remote.unwrap_err().contains("remote"));
        assert!(call(window, cx, "m1", "terminals.read", json!({"id": "t-0-99"})).is_err());
        assert!(call(window, cx, "m1", "terminals.read", json!({})).is_err());
    }

    #[gpui::test]
    fn a_cwd_outside_the_project_is_refused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        // A real project with a real folder beside it: the refusal is about the folder being
        // outside, not about a path that does not exist.
        let root = scratch("outside");
        let project = root.join("app");
        let outside = root.join("other");
        std::fs::create_dir_all(project.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let project = project.to_string_lossy().into_owned();
        let open_in = |cx: &mut TestAppContext, cwd: Value| {
            window
                .update(cx, |shell, _, cx| {
                    shell.mono_terminals_call(
                        "m1",
                        &project,
                        "terminals.open",
                        &json!({"label": "BE", "place": "dock", "reveal": false, "cwd": cwd}),
                        now(),
                        cx,
                    )
                })
                .unwrap()
        };
        let refused = |reply: MonoTerminalReply| match reply {
            MonoTerminalReply::Now(Err(error)) => error,
            _ => panic!("the cwd was accepted"),
        };
        let error = refused(open_in(cx, json!(outside.to_string_lossy())));
        assert!(error.contains("must be inside the project"), "{error}");
        let error = refused(open_in(cx, json!("../other")));
        assert!(error.contains(".."), "{error}");
        let error = refused(open_in(cx, json!("sub/../../other")));
        assert!(error.contains(".."), "{error}");
        let error = refused(open_in(cx, json!("")));
        assert!(error.contains("must not be empty"), "{error}");
        let error = refused(open_in(cx, json!(3)));
        assert!(error.contains("cwd must be text"), "{error}");
        assert_eq!(all_terminals(window, cx), 0);
        // The same call with a folder inside the project goes through.
        let MonoTerminalReply::Now(Ok(done)) = open_in(cx, json!("sub")) else {
            panic!("a folder inside the project was refused");
        };
        assert_eq!(done["reused"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn open_command(label: &str, command: &str) -> Value {
        json!({"label": label, "command": command, "place": "dock"})
    }

    fn declined() -> Result<Value, String> {
        Err(mono_approval::DECLINED.to_owned())
    }

    /// Key bindings, as the running app has them, for the tests that press keys.
    fn bind(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::input::bind(cx);
            crate::popover::bind(cx);
            crate::image_lightbox::bind(cx);
            crate::shell::bind_keys(cx);
            cx.set_reduce_motion(true);
        });
    }

    fn guard_over(cx: &mut TestAppContext) {
        cx.executor()
            .advance_clock(mono_approval::GUARD + Duration::from_millis(50));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_command_waits_for_the_user_and_runs_exactly_what_was_shown(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.open",
            open_command("BE", "npm run dev -- --port 3000"),
        );
        cx.run_until_parked();
        // Showing the dialog opened and wrote nothing.
        assert_eq!(all_terminals(window, cx), 0);
        assert!(rows(window, cx).is_empty());
        assert_eq!(waiting(window, cx), 1);
        window
            .update(cx, |shell, _, _| {
                let ask = shell.mono_approvals.queue.front().unwrap();
                assert_eq!(ask.request.text, "npm run dev -- --port 3000");
                assert_eq!(ask.request.label, "BE");
                assert_eq!(ask.request.cwd, PROJECT);
                assert_eq!(ask.mono_name, "Mono m1");
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Run);
        cx.run_until_parked();
        let id = answer.try_recv().unwrap().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(sent(window, cx, &id), b"npm run dev -- --port 3000\r");
        assert_eq!(waiting(window, cx), 0);
    }

    #[gpui::test]
    fn deny_and_escape_answer_an_error_and_write_nothing(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let first = wait(
            window,
            cx,
            "m1",
            "terminals.open",
            open_command("BE", "rm x"),
        );
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(first.try_recv().unwrap(), declined());
        assert_eq!(all_terminals(window, cx), 0);

        // Escape, as the user presses it with the dialog in front.
        bind(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let second = wait(
            window,
            cx,
            "m1",
            "terminals.open",
            open_command("BE", "rm y"),
        );
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert_eq!(second.try_recv().unwrap(), declined());
        assert_eq!(waiting(window, cx), 0);
        assert_eq!(all_terminals(window, cx), 0);

        // Typing the terminal's text: the same.
        let id = dock(window, cx, "BE");
        let third = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "rm z"}),
        );
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert_eq!(third.try_recv().unwrap(), declined());
        assert!(sent(window, cx, &id).is_empty());
    }

    fn selected(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> usize {
        window
            .update(cx, |s, _, _| s.mono_approvals.queue[0].selected)
            .unwrap()
    }

    #[gpui::test]
    fn enter_denies_until_the_focus_is_moved_to_run_and_the_dialog_is_armed(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        bind(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        // Deny holds the focus first: Enter declines.
        let first = wait(window, cx, "m1", "terminals.open", open_command("BE", "ls"));
        visual.run_until_parked();
        guard_over(cx);
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(first.try_recv().unwrap(), declined());
        assert_eq!(all_terminals(window, cx), 0);

        // Tab, the arrows and Enter in the first moments do nothing at all: the focus stays
        // on Deny and nothing is approved.
        let second = wait(window, cx, "m1", "terminals.open", open_command("BE", "ls"));
        visual.run_until_parked();
        visual.simulate_keystrokes("tab right left shift-tab");
        visual.run_until_parked();
        assert_eq!(
            selected(window, cx),
            0,
            "no key moves the focus while guarded"
        );
        visual.simulate_keystrokes("tab enter");
        visual.run_until_parked();
        assert_eq!(waiting(window, cx), 1, "the guard held");
        assert!(second.try_recv().is_err());
        assert_eq!(all_terminals(window, cx), 0);
        // Once armed, Tab (or an arrow) reaches Run and Enter presses it.
        guard_over(cx);
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        assert_eq!(selected(window, cx), 2, "Tab reached Run");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        let id = second.try_recv().unwrap().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(sent(window, cx, &id), b"ls\r");
    }

    #[gpui::test]
    fn every_key_before_the_dialog_is_armed_starts_the_guard_over(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        bind(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let answer = wait(window, cx, "m1", "terminals.open", open_command("BE", "ls"));
        visual.run_until_parked();
        // A key just before the guard ends: the dialog is not armed when the first timer fires.
        cx.executor()
            .advance_clock(mono_approval::GUARD - Duration::from_millis(100));
        cx.run_until_parked();
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        let armed = |cx: &mut TestAppContext| {
            window
                .update(cx, |s, _, _| s.mono_approvals.queue[0].armed)
                .unwrap()
        };
        assert!(!armed(cx), "the key restarted the guard");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert!(
            answer.try_recv().is_err(),
            "Enter before arming does nothing"
        );
        guard_over(cx);
        assert!(armed(cx));
    }

    #[gpui::test]
    fn enter_never_gives_allow_and_the_keyboard_cannot_focus_it(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        bind(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let answer = wait(window, cx, "m1", "terminals.open", open_command("BE", "ls"));
        visual.run_until_parked();
        guard_over(cx);
        // Walk the whole ring: Deny, Run, Deny, Run. Allow is never the focused one.
        let mut seen = vec![selected(window, cx)];
        for _ in 0..4 {
            visual.simulate_keystrokes("tab");
            visual.run_until_parked();
            seen.push(selected(window, cx));
        }
        assert!(seen.iter().all(|s| *s != 1), "{seen:?}");
        // Even forced onto Allow, Enter does nothing.
        window
            .update(cx, |s, _, _| s.mono_approvals.queue[0].selected = 1)
            .unwrap();
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert!(answer.try_recv().is_err());
        assert_eq!(waiting(window, cx), 1);
        assert_eq!(all_terminals(window, cx), 0);
        // The pointer can still give it.
        visual.refresh().unwrap();
        let allow = visual.debug_bounds("mono-approval-allow").unwrap().center();
        visual.simulate_click(allow, Default::default());
        visual.run_until_parked();
        assert!(answer.try_recv().unwrap().is_ok());
    }

    #[gpui::test]
    fn the_dialog_shows_the_text_and_its_buttons_work_by_click_after_the_guard(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        bind(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let answer = wait(window, cx, "m1", "terminals.open", open_command("BE", "ls"));
        visual.run_until_parked();
        visual.refresh().unwrap();
        for part in [
            "mono-approval-text",
            "mono-approval-deny",
            "mono-approval-allow",
            "mono-approval-run",
        ] {
            assert!(visual.debug_bounds(part).is_some(), "{part}");
        }
        let run = visual.debug_bounds("mono-approval-run").unwrap().center();
        visual.simulate_click(run, Default::default());
        visual.run_until_parked();
        assert_eq!(
            waiting(window, cx),
            1,
            "a click in the guard approves nothing"
        );
        guard_over(cx);
        visual.refresh().unwrap();
        let run = visual.debug_bounds("mono-approval-run").unwrap().center();
        visual.simulate_click(run, Default::default());
        visual.run_until_parked();
        assert!(answer.try_recv().unwrap().is_ok());
        assert_eq!(waiting(window, cx), 0);
        // Deny by click.
        let denied = wait(window, cx, "m1", "terminals.open", open_command("FE", "ls"));
        visual.run_until_parked();
        visual.refresh().unwrap();
        let deny = visual.debug_bounds("mono-approval-deny").unwrap().center();
        visual.simulate_click(deny, Default::default());
        visual.run_until_parked();
        assert_eq!(denied.try_recv().unwrap(), declined());
    }

    #[gpui::test]
    fn allow_remembers_the_exact_string_for_that_mono_only(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1", "m2"]);
        let first = wait(
            window,
            cx,
            "m1",
            "terminals.open",
            open_command("BE", "npm test"),
        );
        choose(window, cx, mono_approval::Choice::Allow);
        assert!(first.try_recv().unwrap().is_ok());
        window
            .update(cx, |shell, _, cx| {
                let monos = shell.mono_manager();
                let m = monos.read(cx).roster.get("m1").unwrap();
                assert_eq!(m.terminal_allow.len(), 1);
                assert!(m.allows_terminal(PROJECT, "command", "npm test"));
                assert!(!m.allows_terminal("/elsewhere", "command", "npm test"));
                assert!(
                    monos
                        .read(cx)
                        .roster
                        .get("m2")
                        .unwrap()
                        .terminal_allow
                        .is_empty()
                );
            })
            .unwrap();
        // The same string runs at once, with no dialog.
        let again = call(
            window,
            cx,
            "m1",
            "terminals.open",
            open_command("FE", "npm test"),
        )
        .unwrap();
        assert_eq!(waiting(window, cx), 0);
        let id = again["id"].as_str().unwrap().to_owned();
        assert_eq!(sent(window, cx, &id), b"npm test\r");
        // One space, one letter or a missing one is another string, and asks.
        for near in ["npm test ", " npm test", "npm  test", "npm Test", "npm tes"] {
            let answer = wait(window, cx, "m1", "terminals.open", open_command("X", near));
            choose(window, cx, mono_approval::Choice::Deny);
            assert_eq!(answer.try_recv().unwrap(), declined(), "{near:?}");
        }
        // Another Mono is asked about the very same string.
        let other = wait(
            window,
            cx,
            "m2",
            "terminals.open",
            open_command("X", "npm test"),
        );
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(other.try_recv().unwrap(), declined());
        // A remembered command is not remembered text.
        let text = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "npm test"}),
        );
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(text.try_recv().unwrap(), declined());
        // Remembered text types without asking, and still presses no key by itself.
        let typed = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "y"}),
        );
        choose(window, cx, mono_approval::Choice::Allow);
        assert!(typed.try_recv().unwrap().is_ok());
        call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "y"}),
        )
        .unwrap();
        assert_eq!(sent(window, cx, &id), b"npm test\ryy");
        assert_eq!(waiting(window, cx), 0);
    }

    #[gpui::test]
    fn input_over_4_kib_or_with_control_bytes_is_refused_before_asking(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let fits = "x".repeat(4096);
        for (action, field) in [("terminals.send", "text"), ("terminals.open", "command")] {
            let build = |value: &str| {
                if field == "text" {
                    json!({"id": id, "text": value})
                } else {
                    open_command("BE2", value)
                }
            };
            for bad in ["x".repeat(4097), "\u{e9}".repeat(2049)] {
                let err = call(window, cx, "m1", action, build(&bad)).unwrap_err();
                assert!(err.contains("4096"), "{action}: {err}");
            }
            let err = call(window, cx, "m1", action, build("")).unwrap_err();
            assert!(err.contains("empty"), "{err}");
            let ok = wait(window, cx, "m1", action, build(&fits));
            choose(window, cx, mono_approval::Choice::Deny);
            assert_eq!(ok.try_recv().unwrap(), declined());
            for bad in [
                "a\u{1b}[2Jb",
                "a\tb",
                "a\rb",
                "\u{7f}",
                "\u{3}",
                "a\u{0}b",
                "a\u{9b}b",
                "a\u{8}b",
            ] {
                let err = call(window, cx, "m1", action, build(bad)).unwrap_err();
                assert!(err.contains("use key for control keys"), "{bad:?}: {err}");
            }
            // Tab is a key, never text: the error says so.
            let err = call(window, cx, "m1", action, build("a\tb")).unwrap_err();
            assert!(err.contains("tab"), "{err}");
        }
        assert!(sent(window, cx, &id).is_empty());
        let both = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "X", "action": "a1", "command": "ls"}),
        );
        assert!(both.unwrap_err().contains("not both"));
        let bad_key = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls", "key": "ctrl-d"}),
        );
        assert!(bad_key.unwrap_err().contains("Unknown key"));
        let none = call(window, cx, "m1", "terminals.send", json!({"id": id}));
        assert!(none.unwrap_err().contains("text or key"));
    }

    #[gpui::test]
    fn multi_line_text_is_a_bracketed_paste_and_refused_when_the_program_did_not_ask_for_one(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        let run = |cx: &mut TestAppContext, input: Value| {
            let answer = wait(window, cx, "m1", "terminals.send", input);
            choose(window, cx, mono_approval::Choice::Run);
            answer.try_recv().unwrap().unwrap()
        };
        // Off: each line would run as it arrives, so it is refused before anyone is asked.
        let err = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "a\nb"}),
        )
        .unwrap_err();
        assert!(err.contains("bracketed paste"), "{err}");
        assert_eq!(waiting(window, cx), 0);
        assert!(sent(window, cx, &id).is_empty());
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| t.fake_output("\x1b[?2004h", cx))
            })
            .unwrap();
        run(cx, json!({"id": id, "text": "c\nd"}));
        assert_eq!(sent(window, cx, &id), b"\x1b[200~c\nd\x1b[201~");
        // One line stays bare in both modes, and Enter can follow in the same call.
        let before = sent(window, cx, &id).len();
        let result = run(cx, json!({"id": id, "text": "ls -la", "key": "enter"}));
        assert_eq!(result["key"], "enter");
        assert_eq!(sent(window, cx, &id)[before..], *b"ls -la\r");
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| t.fake_output("\x1b[?2004l", cx))
            })
            .unwrap();
        let before = sent(window, cx, &id).len();
        run(cx, json!({"id": id, "text": "q"}));
        assert_eq!(sent(window, cx, &id)[before..], *b"q");
    }

    #[gpui::test]
    fn the_paste_mode_changing_between_the_dialog_and_the_write_writes_nothing(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| t.fake_output("\x1b[?2004h", cx))
            })
            .unwrap();
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "a\nb"}),
        );
        // The program turned the mode off after the dialog was shown.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| t.fake_output("\x1b[?2004l", cx))
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Run);
        let err = answer.try_recv().unwrap().unwrap_err();
        assert!(
            err.contains("changed") || err.contains("bracketed"),
            "{err}"
        );
        assert!(sent(window, cx, &id).is_empty());
    }

    #[gpui::test]
    fn a_multi_line_command_for_a_new_terminal_is_refused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let err = call(
            window,
            cx,
            "m1",
            "terminals.open",
            open_command("BE", "echo a\necho b"),
        )
        .unwrap_err();
        assert!(err.contains("bracketed paste"), "{err}");
        assert_eq!((waiting(window, cx), all_terminals(window, cx)), (0, 0));
    }

    #[gpui::test]
    fn pending_approvals_queue_one_dialog_at_a_time_and_answer_in_order(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1", "m2", "m3"]);
        let first = wait(window, cx, "m1", "terminals.open", open_command("A", "one"));
        let second = wait(window, cx, "m2", "terminals.open", open_command("B", "two"));
        let third = wait(
            window,
            cx,
            "m3",
            "terminals.open",
            open_command("C", "three"),
        );
        assert_eq!(waiting(window, cx), 3);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.mono_approvals.queue[0].request.text, "one");
                assert!(shell.mono_approvals.queue[0].shown);
                assert!(!shell.mono_approvals.queue[1].shown, "one dialog at a time");
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Run);
        assert!(first.try_recv().unwrap().is_ok());
        assert!(second.try_recv().is_err() && third.try_recv().is_err());
        window
            .update(cx, |shell, _, _| {
                let front = shell.mono_approvals.queue.front().unwrap();
                assert_eq!(front.request.text, "two");
                assert!(front.shown && !front.armed, "the next one starts guarded");
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(second.try_recv().unwrap(), declined());
        assert!(third.try_recv().is_err());
        choose(window, cx, mono_approval::Choice::Run);
        assert!(third.try_recv().unwrap().is_ok());
        assert_eq!(all_terminals(window, cx), 2);
    }

    #[gpui::test]
    fn a_terminal_closed_while_the_dialog_waits_answers_an_error_and_writes_nothing(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1", "m2"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        let id2 = dock_as(window, cx, "m2", "BE2");
        let a = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        let b = wait(
            window,
            cx,
            "m2",
            "terminals.send",
            json!({"id": id2, "text": "pwd"}),
        );
        window
            .update(cx, |shell, w, cx| {
                shell.remove_terminal(PROJECT, 1, w, cx);
                shell.remove_terminal(PROJECT, 0, w, cx)
            })
            .unwrap();
        // The check that runs while the dialog is up drops both without anyone choosing.
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        for answer in [a, b] {
            let err = answer.try_recv().unwrap().unwrap_err();
            assert!(err.contains("Unknown terminal"), "{err}");
        }
        assert_eq!(waiting(window, cx), 0);
        assert!(
            window
                .update(cx, |_, _, cx| t.read(cx).sent_bytes().is_empty())
                .unwrap()
        );

        // Closed between the last check and the click: approving checks again.
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        let c = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        window
            .update(cx, |shell, w, cx| shell.remove_terminal(PROJECT, 0, w, cx))
            .unwrap();
        choose(window, cx, mono_approval::Choice::Run);
        assert!(
            c.try_recv()
                .unwrap()
                .unwrap_err()
                .contains("Unknown terminal")
        );
        assert!(
            window
                .update(cx, |_, _, cx| t.read(cx).sent_bytes().is_empty())
                .unwrap()
        );
    }

    #[gpui::test]
    fn a_deleted_mono_or_an_unlisted_project_runs_nothing(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1", "m2"]);
        let gone = wait(window, cx, "m1", "terminals.open", open_command("A", "one"));
        let moved = wait(window, cx, "m2", "terminals.open", open_command("B", "two"));
        window
            .update(cx, |shell, _, cx| {
                shell.mono_manager().update(cx, |monos, _| {
                    monos.roster.monos.retain(|m| m.id != "m1");
                    monos.roster.get_mut("m2").unwrap().projects = vec!["/elsewhere".into()];
                })
            })
            .unwrap();
        // The user clicks Run on the first before the check had a chance to drop it.
        choose(window, cx, mono_approval::Choice::Run);
        let err = gone.try_recv().unwrap().unwrap_err();
        assert!(err.contains("no longer available"), "{err}");
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        assert!(
            moved
                .try_recv()
                .unwrap()
                .unwrap_err()
                .contains("no longer available")
        );
        assert_eq!((waiting(window, cx), all_terminals(window, cx)), (0, 0));
        // A Mono that is not on the roster is not asked about at all.
        let unknown = ask(
            window,
            cx,
            "ghost",
            "terminals.open",
            open_command("A", "one"),
        );
        let MonoTerminalReply::Now(Err(err)) = unknown else {
            panic!("a Mono that is gone cannot be asked about");
        };
        assert!(err.contains("no longer available"));
    }

    #[gpui::test]
    fn an_approval_after_the_call_was_abandoned_runs_nothing(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let answer = wait(window, cx, "m1", "terminals.open", open_command("A", "one"));
        drop(answer);
        choose(window, cx, mono_approval::Choice::Run);
        assert_eq!(all_terminals(window, cx), 0, "no terminal, no command");
        let id = dock(window, cx, "BE");
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls", "key": "enter"}),
        );
        drop(answer);
        choose(window, cx, mono_approval::Choice::Allow);
        assert!(sent(window, cx, &id).is_empty());
        assert_eq!(waiting(window, cx), 0);
    }

    #[gpui::test]
    fn an_unanswered_dialog_expires_before_the_bridge_gives_up(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1", "m2"]);
        let id = dock(window, cx, "BE");
        let id2 = dock_as(window, cx, "m2", "BE2");
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        let later = wait(
            window,
            cx,
            "m2",
            "terminals.send",
            json!({"id": id2, "text": "pwd"}),
        );
        let wait_for = monocode_core::mono::bridge::APPROVAL_WAIT;
        assert!(wait_for < monocode_core::mono::bridge::wait_for("terminals.send"));
        cx.executor()
            .advance_clock(wait_for - Duration::from_secs(1));
        cx.run_until_parked();
        assert!(answer.try_recv().is_err(), "still waiting just before");
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        for answer in [answer, later] {
            let err = answer.try_recv().unwrap().unwrap_err();
            assert!(err.contains("did not answer in time"), "{err}");
        }
        assert_eq!(waiting(window, cx), 0);
        assert!(sent(window, cx, &id).is_empty());
    }

    #[gpui::test]
    fn free_input_is_refused_before_asking_when_it_could_never_run(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let user = user_terminal(window, cx);
        let user_id = rows(window, cx)[0]["id"].as_str().unwrap().to_owned();
        let err = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": user_id, "text": "ls"}),
        )
        .unwrap_err();
        assert!(err.contains("user opened"), "{err}");
        let agent = dock(window, cx, "AI");
        let t = terminal(window, cx, &agent);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.foreground = Some("claude".into()))
            })
            .unwrap();
        let err = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": agent, "text": "ls"}),
        )
        .unwrap_err();
        assert!(err.contains("agent CLI"), "{err}");
        for cwd in ["/tmp", "../x"] {
            let mut input = open_command("X", "ls");
            input["cwd"] = json!(cwd);
            assert!(
                call(window, cx, "m1", "terminals.open", input).is_err(),
                "{cwd}"
            );
        }
        assert_eq!(waiting(window, cx), 0);
        assert!(
            window
                .update(cx, |_, _, cx| user.read(cx).sent_bytes().is_empty())
                .unwrap()
        );
    }

    #[gpui::test]
    fn a_saved_action_still_runs_without_asking(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        window
            .update(cx, |shell, _, _| {
                shell.workspace.actions.insert(
                    PROJECT.into(),
                    vec![ProjectAction {
                        id: "a1".into(),
                        name: "Dev".into(),
                        command: "npm run dev".into(),
                        ..Default::default()
                    }],
                );
            })
            .unwrap();
        let opened = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "action": "a1", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(waiting(window, cx), 0);
        assert_eq!(
            sent(window, cx, opened["id"].as_str().unwrap()),
            b"npm run dev\r"
        );
    }

    #[gpui::test]
    fn the_foreground_changing_between_the_dialog_and_the_write_writes_nothing(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        window
            .update(cx, |shell, _, _| {
                let shown = shell.mono_approvals.queue[0].request.seen.clone().unwrap();
                assert_eq!(shown.front, Front::Shell("zsh".into()));
            })
            .unwrap();
        // A program took the terminal after the dialog was shown: the text would go to it.
        set_front(window, cx, &t, program("vite"));
        choose(window, cx, mono_approval::Choice::Run);
        let err = answer.try_recv().unwrap().unwrap_err();
        assert!(err.contains("changed"), "{err}");
        assert!(sent(window, cx, &id).is_empty());
        // An unchanged terminal is written to.
        set_front(window, cx, &t, None);
        let again = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        choose(window, cx, mono_approval::Choice::Run);
        assert!(again.try_recv().unwrap().is_ok());
        assert_eq!(sent(window, cx, &id), b"ls");
    }

    #[gpui::test]
    fn the_dialog_names_the_program_and_is_honest_about_the_folder(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        bind(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        set_front(window, cx, &t, program("vite"));
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "print(1)", "key": "enter"}),
        );
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("mono-approval-program").is_some());
        window
            .update(cx, |shell, _, _| {
                let ask = &shell.mono_approvals.queue[0].request;
                assert_eq!(ask.seen.as_ref().unwrap().front.name(), "vite");
                // No live folder to read here, so the dialog says where it started.
                assert!(!ask.cwd_live);
                assert_eq!(ask.cwd, PROJECT);
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(answer.try_recv().unwrap(), declined());
        // A new terminal for a command has no program to name yet.
        let fresh = wait(
            window,
            cx,
            "m1",
            "terminals.open",
            open_command("NEW", "ls"),
        );
        window
            .update(cx, |shell, _, _| {
                assert!(shell.mono_approvals.queue[0].request.seen.is_none());
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(fresh.try_recv().unwrap(), declined());
    }

    #[gpui::test]
    fn a_live_folder_outside_the_project_is_refused_and_one_that_moved_writes_nothing(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.fake.cwd = Some("/tmp".into()))
            })
            .unwrap();
        let err = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        )
        .unwrap_err();
        assert!(err.contains("outside the project"), "{err}");
        assert_eq!(waiting(window, cx), 0);
        // The shell `cd`s out after the dialog was shown.
        window
            .update(cx, |_, _, cx| t.update(cx, |t, _| t.fake.cwd = None))
            .unwrap();
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.fake.cwd = Some("/tmp".into()))
            })
            .unwrap();
        choose(window, cx, mono_approval::Choice::Run);
        assert!(answer.try_recv().unwrap().is_err());
        assert!(sent(window, cx, &id).is_empty());
    }

    #[gpui::test]
    fn an_idle_terminal_standing_in_another_folder_is_not_reused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let first = dock(window, cx, "BE");
        let t = terminal(window, cx, &first);
        // The label matches and the shell is idle, but it is not in the project's folder.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.cwd = format!("{PROJECT}/packages/api"))
            })
            .unwrap();
        let again = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(again["reused"], false);
        assert_ne!(again["id"].as_str(), Some(first.as_str()));
        // A program in front, according to the live check, is not idle even if the poll
        // says nothing.
        let second = again["id"].as_str().unwrap().to_owned();
        let t2 = terminal(window, cx, &second);
        set_front(window, cx, &t2, program("vite"));
        let third = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(third["reused"], false);
    }

    #[gpui::test]
    fn a_late_approval_writes_nothing(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let late = Instant::now()
            .checked_sub(monocode_core::mono::bridge::APPROVAL_WAIT + Duration::from_secs(1))
            .expect("the clock is far enough from its start");
        let answer = match ask_via(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
            Via {
                session: 0,
                received: late,
            },
        ) {
            MonoTerminalReply::Later(answer) => answer,
            MonoTerminalReply::Now(result) => panic!("did not wait: {result:?}"),
        };
        // The user clicks after the Mono's own wait is over.
        choose(window, cx, mono_approval::Choice::Allow);
        let err = answer.try_recv().unwrap().unwrap_err();
        assert!(err.contains("did not answer in time"), "{err}");
        assert!(sent(window, cx, &id).is_empty());
        // It was not remembered either.
        window
            .update(cx, |shell, _, cx| {
                let monos = shell.mono_manager();
                let m = monos.read(cx).roster.get("m1").unwrap();
                assert!(m.terminal_allow.is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn stopping_the_monos_turn_cancels_its_waiting_dialog(cx: &mut TestAppContext) {
        let (window, _, chat) = window(cx);
        add_monos(window, cx, &["m1"]);
        let view = window
            .update(cx, |shell, _, cx| {
                let view = shell.sessions.get(&chat).unwrap().clone();
                view.update(cx, |v, _| v.busy = true);
                shell
                    .mono_manager()
                    .update(cx, |monos, _| monos.adopt_chat("m1", view.clone()));
                view
            })
            .unwrap();
        let session = window
            .update(cx, |_, _, cx| view.read(cx).summary.id)
            .unwrap();
        let id = dock(window, cx, "BE");
        let via = Via {
            session,
            received: Instant::now(),
        };
        let ask_text = |cx: &mut TestAppContext| match ask_via(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
            via,
        ) {
            MonoTerminalReply::Later(answer) => answer,
            MonoTerminalReply::Now(result) => panic!("did not wait: {result:?}"),
        };
        // While the turn runs the dialog stays.
        let answer = ask_text(cx);
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        assert!(answer.try_recv().is_err());
        assert_eq!(waiting(window, cx), 1);
        // The user stops the turn: the dialog goes away on its own and nothing is written.
        window
            .update(cx, |_, _, cx| view.update(cx, |v, _| v.busy = false))
            .unwrap();
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        let err = answer.try_recv().unwrap().unwrap_err();
        assert!(err.contains("turn was stopped"), "{err}");
        assert_eq!(waiting(window, cx), 0);
        assert!(sent(window, cx, &id).is_empty());
        // A click in the half second before the check ran is refused as well.
        window
            .update(cx, |_, _, cx| view.update(cx, |v, _| v.busy = true))
            .unwrap();
        let second = ask_text(cx);
        window
            .update(cx, |_, _, cx| view.update(cx, |v, _| v.busy = false))
            .unwrap();
        choose(window, cx, mono_approval::Choice::Run);
        assert!(
            second
                .try_recv()
                .unwrap()
                .unwrap_err()
                .contains("turn was stopped")
        );
        assert!(sent(window, cx, &id).is_empty());
    }

    #[gpui::test]
    fn a_new_turn_or_a_new_chat_cancels_the_old_dialog(cx: &mut TestAppContext) {
        let (window, _, chat) = window(cx);
        add_monos(window, cx, &["m1"]);
        let view = window
            .update(cx, |shell, _, cx| {
                let view = shell.sessions.get(&chat).unwrap().clone();
                view.update(cx, |v, _| v.busy = true);
                shell
                    .mono_manager()
                    .update(cx, |monos, _| monos.adopt_chat("m1", view.clone()));
                view
            })
            .unwrap();
        let session = window
            .update(cx, |_, _, cx| view.read(cx).summary.id)
            .unwrap();
        let id = dock(window, cx, "BE");
        let ask_text = |cx: &mut TestAppContext| match ask_via(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
            Via {
                session,
                received: Instant::now(),
            },
        ) {
            MonoTerminalReply::Later(answer) => answer,
            MonoTerminalReply::Now(result) => panic!("did not wait: {result:?}"),
        };
        // The user stops the turn and starts another one inside the watch window: the busy
        // flag is on again, but it is not the turn that asked.
        let answer = ask_text(cx);
        window
            .update(cx, |_, _, cx| {
                view.update(cx, |v, _| {
                    v.busy = false;
                    v.busy = true;
                    v.submission_generation = v.submission_generation.wrapping_add(1);
                })
            })
            .unwrap();
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        let err = answer.try_recv().unwrap().unwrap_err();
        assert!(err.contains("turn was stopped"), "{err}");
        assert!(sent(window, cx, &id).is_empty());
        // A different chat in place of the one that asked: the same.
        let answer = ask_text(cx);
        window
            .update(cx, |_, _, cx| view.update(cx, |v, _| v.summary.id += 1000))
            .unwrap();
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        let err = answer.try_recv().unwrap().unwrap_err();
        assert!(err.contains("turn was stopped"), "{err}");
        assert!(sent(window, cx, &id).is_empty());
    }

    #[gpui::test]
    fn a_mono_has_one_request_waiting_at_a_time(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1", "m2"]);
        let first = wait(window, cx, "m1", "terminals.open", open_command("A", "one"));
        let refused = ask(window, cx, "m1", "terminals.open", open_command("B", "two"));
        let MonoTerminalReply::Now(Err(err)) = refused else {
            panic!("a second request from the same Mono must be refused at once");
        };
        assert!(err.contains("previous request"), "{err}");
        assert_eq!(waiting(window, cx), 1);
        // Another Mono has its own place in the queue.
        let other = wait(
            window,
            cx,
            "m2",
            "terminals.open",
            open_command("C", "three"),
        );
        assert_eq!(waiting(window, cx), 2);
        // Once the first is answered the Mono may ask again.
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(first.try_recv().unwrap(), declined());
        let again = wait(window, cx, "m1", "terminals.open", open_command("B", "two"));
        assert_eq!(waiting(window, cx), 2);
        assert!(other.try_recv().is_err() && again.try_recv().is_err());
        assert_eq!(all_terminals(window, cx), 0);
    }

    #[gpui::test]
    fn read_masks_the_whole_scrollback_before_it_cuts_the_tail(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| {
                    // A key whose first line is far above the lines asked for, and whose end
                    // was never printed.
                    t.fake_output("-----BEGIN PRIVATE KEY-----\r\n", cx);
                    for n in 0..40 {
                        t.fake_output(&format!("MIIEvQIBADANBgkqhkiG9w0BAQEF{n:02}\r\n"), cx);
                    }
                })
            })
            .unwrap();
        let read = call(
            window,
            cx,
            "m1",
            "terminals.read",
            json!({"id": id, "lines": 5}),
        )
        .unwrap();
        let text = read["text"].as_str().unwrap();
        assert!(!text.contains("MIIEv"), "{text}");
        assert!(text.contains("redacted"), "{text}");
    }

    #[gpui::test]
    fn labels_with_invisible_characters_are_refused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        for bad in [
            "B\u{200b}E",
            "BE\u{202e}",
            "\u{2066}BE",
            "B\u{feff}E",
            "B\u{3164}E",
            "B\u{7}E",
            "B\nE",
        ] {
            let err = call(
                window,
                cx,
                "m1",
                "terminals.open",
                json!({"label": bad, "place": "dock"}),
            )
            .unwrap_err();
            assert!(err.contains("label"), "{bad:?}: {err}");
        }
        assert_eq!(all_terminals(window, cx), 0);
        // Accented and non-Latin text is a fine label.
        for ok in ["Máy chủ", "日本語", "BE 🚀"] {
            call(
                window,
                cx,
                "m1",
                "terminals.open",
                json!({"label": ok, "place": "dock"}),
            )
            .unwrap();
        }
    }

    #[gpui::test]
    fn allow_is_remembered_only_after_the_checked_write_went_through(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        set_front(window, cx, &t, program("vite"));
        choose(window, cx, mono_approval::Choice::Allow);
        assert!(answer.try_recv().unwrap().is_err());
        window
            .update(cx, |shell, _, cx| {
                let monos = shell.mono_manager();
                assert!(
                    monos
                        .read(cx)
                        .roster
                        .get("m1")
                        .unwrap()
                        .terminal_allow
                        .is_empty()
                );
            })
            .unwrap();
        // A write that goes through is remembered, for this project.
        set_front(window, cx, &t, None);
        let again = wait(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        );
        choose(window, cx, mono_approval::Choice::Allow);
        assert!(again.try_recv().unwrap().is_ok());
        window
            .update(cx, |shell, _, cx| {
                let monos = shell.mono_manager();
                let m = monos.read(cx).roster.get("m1").unwrap().clone();
                assert!(m.allows_terminal(PROJECT, "text", "ls"));
                assert!(!m.allows_terminal("/another", "text", "ls"));
            })
            .unwrap();
    }

    #[gpui::test]
    fn let_mono_read_switches_the_terminal_the_menu_was_opened_for(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let first = user_terminal(window, cx);
        let second = user_terminal(window, cx);
        // The menu of the second tab was opened; the tabs moved before the item was chosen,
        // so index 0 is now another terminal.
        let toggle = window
            .update(cx, |_, _, cx| {
                Shell::mono_read_item(&second, second.read(cx).mono_read)
            })
            .unwrap();
        let MenuItem::Action(action) = toggle else {
            panic!("an action");
        };
        window
            .update(cx, |shell, w, cx| {
                shell.run_terminal_tab_menu(0, &action.id, w, cx)
            })
            .unwrap();
        window
            .update(cx, |_, _, cx| {
                assert!(
                    !first.read(cx).mono_read,
                    "the tab at the index is untouched"
                );
                assert!(second.read(cx).mono_read);
            })
            .unwrap();
        // A terminal that is gone leaves the others alone.
        window
            .update(cx, |shell, _, cx| {
                shell.toggle_mono_read(u64::MAX, cx);
                assert!(!first.read(cx).mono_read);
            })
            .unwrap();
    }

    #[gpui::test]
    fn ids_carry_a_random_part_per_launch_so_old_ones_name_nothing(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 3, "{id}");
        assert_eq!(parts[0], "t");
        assert_eq!(parts[1].len(), 4);
        assert!(parts[1].chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert_eq!(parts[2], "1");
        // The id an earlier launch handed out is not this one's.
        let err = call(window, cx, "m1", "terminals.read", json!({"id": "t-1"})).unwrap_err();
        assert!(err.contains("Unknown terminal"), "{err}");
    }

    #[gpui::test]
    fn the_dialog_pins_the_last_line_and_warns_about_what_it_cannot_show(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1"]);
        bind(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let short = wait(window, cx, "m1", "terminals.open", open_command("A", "ls"));
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("mono-approval-last").is_none());
        assert!(visual.debug_bounds("mono-approval-non-ascii").is_none());
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(short.try_recv().unwrap(), declined());
        let long = format!("echo {}; rm -rf x", "y".repeat(600));
        let answer = wait(window, cx, "m1", "terminals.open", open_command("B", &long));
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("mono-approval-last").is_some());
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(answer.try_recv().unwrap(), declined());
        let accented = wait(
            window,
            cx,
            "m1",
            "terminals.open",
            open_command("C", "echo caf\u{e9}"),
        );
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("mono-approval-non-ascii").is_some());
        choose(window, cx, mono_approval::Choice::Deny);
        assert_eq!(accented.try_recv().unwrap(), declined());
    }

    #[test]
    fn a_live_folder_is_inside_when_it_is_the_project_or_below_it() {
        let base = scratch("live-cwd");
        let project = base.join("app");
        let inside = project.join("src");
        let outside = base.join("other");
        for dir in [&inside, &outside] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let project = project.to_string_lossy().into_owned();
        assert!(live_cwd_inside(&project, &project));
        assert!(live_cwd_inside(&project, &inside.to_string_lossy()));
        assert!(!live_cwd_inside(&project, &outside.to_string_lossy()));
        assert!(!live_cwd_inside(&project, &format!("{project}/../other")));
        assert!(!live_cwd_inside(&project, &format!("{project}/gone")));
        assert!(!live_cwd_inside(&project, "/"));
    }

    fn bounds(
        visual: &mut gpui::VisualTestContext,
        selector: String,
    ) -> Option<gpui::Bounds<Pixels>> {
        // Selectors live for the whole test run, like the literals they stand in for.
        visual.debug_bounds(Box::leak(selector.into_boxed_str()))
    }

    /// The pane a terminal id stands for.
    fn pane_of(window: WindowHandle<Shell>, cx: &mut TestAppContext, id: &str) -> i64 {
        window
            .update(cx, |shell, _, _| {
                match shell.find_mono_terminal("m1", PROJECT, id).unwrap().spot {
                    Spot::Pane(pane) => pane,
                    Spot::Dock(_) => panic!("{id} is in the dock"),
                }
            })
            .unwrap()
    }

    fn layout(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        preset: &str,
        ids: &[&str],
    ) -> Result<Value, String> {
        call(
            window,
            cx,
            "m1",
            "terminals.layout",
            json!({"preset": preset, "ids": ids, "reveal": false}),
        )
    }

    /// A chat tab with the title placement and a Mono "m1" that works on the project.
    fn logs_setup(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64) {
        let (window, _, chat) = window(cx);
        add_monos(window, cx, &["m1", "m2"]);
        window
            .update(cx, |shell, w, cx| {
                shell.workspace.preferences.set_terminal_placement("title");
                shell.open(chat, w, cx);
            })
            .unwrap();
        (window, chat)
    }

    #[gpui::test]
    fn layout_moves_the_dock_and_other_tabs_into_one_logs_tab_and_reuses_it(
        cx: &mut TestAppContext,
    ) {
        let (window, chat) = logs_setup(cx);
        let be = dock(window, cx, "BE");
        let fe = open(
            window,
            cx,
            json!({"label": "FE", "place": "split", "reveal": false}),
        );
        let db = dock(window, cx, "DB");
        let tabs_before = window.update(cx, |s, _, _| s.workspace.tabs.len()).unwrap();
        let done = layout(window, cx, "columns", &[&be, &fe]).unwrap();
        assert_eq!(done["tab"], "Logs monocode");
        assert_eq!(done["ids"], json!([be, fe]));
        let (be_pane, fe_pane) = (pane_of(window, cx, &be), pane_of(window, cx, &fe));
        window
            .update(cx, |shell, _, cx| {
                let w = &shell.workspace;
                let tab = w.tab_of(be_pane).unwrap();
                assert_eq!(w.tab_of(fe_pane), Some(tab));
                assert_eq!(w.layout(tab).leaf_ids(), vec![be_pane, fe_pane]);
                assert!(matches!(
                    w.layout(tab),
                    crate::pane_layout::PaneNode::Split { dir: SplitDir::Right, ref sizes, .. }
                        if sizes == &vec![0.5, 0.5]
                ));
                // The chat keeps its own tab; the dock lost BE and kept DB.
                assert_ne!(w.tab_of(chat), Some(tab));
                assert_eq!(
                    w.tabs.len(),
                    tabs_before,
                    "FE's own tab became the Logs tab"
                );
                assert_eq!(w.docks[PROJECT].terminals.len(), 1);
                assert_eq!(shell.terminals[PROJECT].len(), 1);
                assert_eq!(w.logs_title(tab), Some("Logs monocode"));
                let copy = shell.tab_copy(tab, cx);
                assert_eq!(copy.headline, "Logs monocode");
                assert_eq!(copy.panes.len(), 1);
                assert_eq!(copy.panes[0].title, "Logs monocode");
                assert!(copy.panes[0].by_mono);
                // The user's chat is still the active pane.
                assert_eq!(w.active, Some(chat));
            })
            .unwrap();
        // The registry follows the live shells.
        assert_eq!(rows(window, cx).len(), 3);
        // A second layout reuses the tab: DB joins, no tab is added.
        let tab_count = window.update(cx, |s, _, _| s.workspace.tabs.len()).unwrap();
        layout(window, cx, "grid", &[&be, &fe, &db]).unwrap();
        let db_pane = pane_of(window, cx, &db);
        window
            .update(cx, |shell, _, _| {
                let w = &shell.workspace;
                assert_eq!(w.tabs.len(), tab_count);
                let tab = w.tab_of(db_pane).unwrap();
                assert_eq!(w.layout(tab).leaf_ids(), vec![be_pane, fe_pane, db_pane]);
                assert_eq!(w.logs_title(tab), Some("Logs monocode"));
                assert!(shell.workspace.docks[PROJECT].terminals.is_empty());
            })
            .unwrap();
        // Leaving a terminal out of a later layout gives it a tab of its own.
        layout(window, cx, "rows", &[&be, &db]).unwrap();
        window
            .update(cx, |shell, _, _| {
                let w = &shell.workspace;
                assert_eq!(w.tabs.len(), tab_count + 1);
                assert_eq!(w.tab_of(be_pane), w.tab_of(db_pane));
                assert_ne!(w.tab_of(fe_pane), w.tab_of(be_pane));
                assert_eq!(
                    w.layout(w.tab_of(fe_pane).unwrap()).leaf_ids(),
                    vec![fe_pane]
                );
                assert_eq!(
                    w.logs_title(w.tab_of(be_pane).unwrap()),
                    Some("Logs monocode")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_split_terminal_joins_the_logs_tab_and_layout_refuses_what_is_not_the_monos(
        cx: &mut TestAppContext,
    ) {
        let (window, _chat) = logs_setup(cx);
        let be = dock(window, cx, "BE");
        let fe = dock(window, cx, "FE");
        layout(window, cx, "columns", &[&be, &fe]).unwrap();
        let joined = open(
            window,
            cx,
            json!({"label": "QA", "place": "split", "reveal": false}),
        );
        let (be_pane, qa_pane) = (pane_of(window, cx, &be), pane_of(window, cx, &joined));
        window
            .update(cx, |shell, _, _| {
                let w = &shell.workspace;
                assert_eq!(w.tab_of(qa_pane), w.tab_of(be_pane));
                assert_eq!(
                    w.logs_title(w.tab_of(qa_pane).unwrap()),
                    Some("Logs monocode")
                );
            })
            .unwrap();
        // Other Monos' and the user's terminals, unknown ids and bad lists move nothing.
        let theirs = call(
            window,
            cx,
            "m2",
            "terminals.open",
            json!({"label": "theirs", "place": "dock", "reveal": false}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let user = user_terminal(window, cx);
        let user_id = rows(window, cx)
            .into_iter()
            .find(|r| r["owner"] == "user")
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .unwrap();
        assert_eq!(terminal(window, cx, &user_id), user);
        let snapshot = |cx: &mut TestAppContext| {
            window
                .update(cx, |s, _, _| {
                    (
                        serde_json::to_string(&s.workspace).unwrap(),
                        s.terminals[PROJECT].len(),
                    )
                })
                .unwrap()
        };
        let before = snapshot(cx);
        let seven: Vec<String> = (0..7).map(|n| format!("t-{n}")).collect();
        let seven: Vec<&str> = seven.iter().map(String::as_str).collect();
        let cases: [(&str, Vec<&str>); 7] = [
            ("columns", vec![&be, &theirs]),
            ("columns", vec![&user_id]),
            ("columns", vec![&be, "t-99"]),
            ("columns", vec![]),
            ("columns", seven),
            ("columns", vec![&be, &be]),
            ("stack", vec![&be]),
        ];
        for (preset, ids) in cases {
            assert!(
                layout(window, cx, preset, &ids).is_err(),
                "{preset} {ids:?}"
            );
        }
        assert_eq!(snapshot(cx), before);
    }

    #[gpui::test]
    fn layout_keeps_the_keyboard_and_the_mono_view_where_they_were(cx: &mut TestAppContext) {
        let (window, _chat) = logs_setup(cx);
        let be = dock(window, cx, "BE");
        let fe = dock(window, cx, "FE");
        window
            .update(cx, |shell, w, cx| {
                shell
                    .mono_manager()
                    .update(cx, |monos, cx| monos.open("m1", w, cx))
            })
            .unwrap();
        let snapshot = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, w, cx| {
                    (
                        w.focused(cx),
                        shell.navigation_revision,
                        shell.workspace.active,
                        shell.mono_manager().read(cx).active.clone(),
                    )
                })
                .unwrap()
        };
        let before = snapshot(cx);
        assert_eq!(before.3.as_deref(), Some("m1"));
        layout(window, cx, "grid", &[&be, &fe]).unwrap();
        assert_eq!(snapshot(cx), before);
        // Revealing selects the tab but leaves the keyboard and the Mono's view.
        call(
            window,
            cx,
            "m1",
            "terminals.layout",
            json!({"preset": "rows", "ids": [be, fe], "reveal": true}),
        )
        .unwrap();
        let after = snapshot(cx);
        assert_eq!((after.0, after.1, after.3), (before.0, before.1, before.3));
        assert_eq!(after.2, Some(pane_of(window, cx, &be)));
    }

    #[gpui::test]
    fn the_mono_mark_is_on_mono_terminals_only(cx: &mut TestAppContext) {
        let (window, chat) = logs_setup(cx);
        cx.update(|cx| {
            bind_keys(cx);
            crate::terminal::bind(cx);
            cx.set_reduce_motion(true);
        });
        let be = open(
            window,
            cx,
            json!({"label": "BE", "place": "dock", "reveal": true}),
        );
        let _user = user_terminal(window, cx);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        // Dock tabs: the Mono's first, the user's second.
        assert!(visual.debug_bounds("terminal-mono-mark-0").is_some());
        assert!(visual.debug_bounds("terminal-mono-mark-1").is_none());
        // Pane headers: the Mono's terminal, not the chat beside it.
        let fe = open(
            window,
            cx,
            json!({"label": "FE", "place": "split", "reveal": true}),
        );
        visual.run_until_parked();
        let fe_pane = pane_of(window, cx, &fe);
        // The user drags their chat beside it: the split's headers tell the two apart.
        window
            .update(cx, |shell, _, cx| {
                shell.workspace.place(chat, fe_pane, Edge::Right);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(bounds(&mut visual, format!("split-mono-mark-{fe_pane}")).is_some());
        assert!(bounds(&mut visual, format!("split-mono-mark-{chat}")).is_none());
        // The Logs tab's title carries it too; the chat's tab does not.
        call(
            window,
            cx,
            "m1",
            "terminals.layout",
            json!({"preset": "columns", "ids": [be, fe], "reveal": true}),
        )
        .unwrap();
        visual.run_until_parked();
        let (focus, label) = window
            .update(cx, |shell, _, cx| {
                let tab = shell.workspace.tab_of(fe_pane).unwrap();
                (
                    shell.workspace.tab_focus(tab),
                    shell.tab_copy(tab, cx).headline,
                )
            })
            .unwrap();
        assert_eq!(label, "Logs monocode");
        assert!(bounds(&mut visual, format!("title-tab-mono-mark-{focus}")).is_some());
        assert!(bounds(&mut visual, format!("title-tab-mono-mark-{chat}")).is_none());
    }

    /// The app started again from `saved`, with the same sessions as `window()` has.
    fn restarted(cx: &mut TestAppContext, saved: String) -> WindowHandle<Shell> {
        let store = monocode_core::store::Store::open(Path::new(":memory:")).unwrap();
        let a = store.create("codex", "/work/another-project").unwrap();
        let b = store.create("claude", "/work/monocode").unwrap();
        let store = std::sync::Arc::new(std::sync::Mutex::new(store));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Shell::new(store, vec![b, a], PROJECT.into(), Some(saved), cx))
            })
            .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        window
    }

    fn saved(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> String {
        window
            .update(cx, |shell, _, _| {
                serde_json::to_string(&shell.workspace).unwrap()
            })
            .unwrap()
    }

    /// A Mono's BE (action, dock) and FE (approved free command, pane) before a restart.
    fn before_restart(cx: &mut TestAppContext) -> String {
        let (window, chat) = logs_setup(cx);
        window
            .update(cx, |shell, _, _| {
                shell.workspace.actions.insert(
                    PROJECT.into(),
                    vec![ProjectAction {
                        id: "a1".into(),
                        name: "Dev".into(),
                        command: "npm run dev".into(),
                        ..Default::default()
                    }],
                );
            })
            .unwrap();
        let be = open(
            window,
            cx,
            json!({"label": "BE", "action": "a1", "place": "dock"}),
        );
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "FE", "command": "npm run web", "place": "split", "reveal": false}),
        );
        choose(window, cx, mono_approval::Choice::Run);
        let fe = answer.try_recv().unwrap().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(sent(window, cx, &fe), b"npm run web\r");
        let _ = (chat, be);
        saved(window, cx)
    }

    #[gpui::test]
    fn a_restart_restores_mono_terminals_stopped_and_runs_nothing(cx: &mut TestAppContext) {
        let json = before_restart(cx);
        let window = restarted(cx, json);
        add_monos(window, cx, &["m1", "m2"]);
        let list = rows(window, cx);
        let labels: Vec<_> = list.iter().map(|r| r["label"].as_str().unwrap()).collect();
        assert_eq!(labels.len(), 2, "{list:?}");
        assert!(labels.contains(&"BE") && labels.contains(&"FE"));
        assert!(list.iter().all(|r| r["owner"] == "mono"));
        assert!(list.iter().all(|r| r["running"] == false));
        // Another Mono sees none of them.
        assert!(
            call(window, cx, "m2", "terminals.list", json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        for row in &list {
            let id = row["id"].as_str().unwrap();
            assert!(sent(window, cx, id).is_empty(), "{id} ran something");
            let t = terminal(window, cx, id);
            window
                .update(cx, |_, _, cx| {
                    let t = t.read(cx);
                    assert!(t.by_mono);
                    let run = t.run_again.clone().unwrap();
                    let (kind, text) = match row["label"].as_str().unwrap() {
                        "BE" => ("action", "npm run dev"),
                        _ => ("command", "npm run web"),
                    };
                    assert_eq!((run.kind.as_str(), run.text.as_str()), (kind, text));
                })
                .unwrap();
        }
        // Nothing was approved on the Mono's behalf either.
        assert_eq!(waiting(window, cx), 0);
    }

    #[gpui::test]
    fn run_again_runs_the_stored_command_once_when_the_user_presses_it(cx: &mut TestAppContext) {
        let json = before_restart(cx);
        let window = restarted(cx, json);
        add_monos(window, cx, &["m1"]);
        cx.update(|cx| {
            bind_keys(cx);
            crate::terminal::bind(cx);
            cx.set_reduce_motion(true);
        });
        let list = rows(window, cx);
        let be = list.iter().find(|r| r["label"] == "BE").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let fe = list.iter().find(|r| r["label"] == "FE").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        // The dock terminal's bar is the one in view; the pane's tab is not selected.
        let button = visual
            .debug_bounds("terminal-run-again")
            .expect("the bar over a restored terminal");
        assert!(sent(window, cx, &be).is_empty());
        visual.simulate_click(button.center(), gpui::Modifiers::default());
        visual.run_until_parked();
        // The user's press is the approval: no dialog, and exactly the stored command.
        assert_eq!(waiting(window, cx), 0);
        assert_eq!(sent(window, cx, &be), b"npm run dev\r");
        visual.run_until_parked();
        assert_eq!(sent(window, cx, &be), b"npm run dev\r");
        // Its bar is gone; the other restored terminal still offers its own and ran nothing.
        let (be_t, fe_t) = (terminal(window, cx, &be), terminal(window, cx, &fe));
        window
            .update(cx, |_, _, cx| {
                assert!(be_t.read(cx).run_again.is_none());
                assert!(fe_t.read(cx).run_again.is_some());
            })
            .unwrap();
        assert!(sent(window, cx, &fe).is_empty());
        // It stays saved for the next restart.
        window
            .update(cx, |shell, _, _| {
                let meta = shell.workspace.docks[PROJECT].mono_meta(0).unwrap();
                assert_eq!(meta.run.as_ref().unwrap().text, "npm run dev");
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_mono_reopens_a_restored_label_and_a_free_command_still_asks(cx: &mut TestAppContext) {
        let json = before_restart(cx);
        let window = restarted(cx, json);
        add_monos(window, cx, &["m1", "m2"]);
        let list = rows(window, cx);
        let id_of_label = |label: &str| {
            list.iter().find(|r| r["label"] == label).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let (be, fe) = (id_of_label("BE"), id_of_label("FE"));
        // A saved action runs again at once, in the same terminal.
        let again = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "action": "a1", "place": "dock", "reveal": false}),
        )
        .unwrap();
        assert_eq!(
            (again["id"].as_str(), again["reused"].as_bool()),
            (Some(be.as_str()), Some(true))
        );
        assert_eq!(sent(window, cx, &be), b"npm run dev\r");
        // The free command that was approved last time is not approved now.
        let answer = wait(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "FE", "command": "npm run web", "place": "split", "reveal": false}),
        );
        assert!(sent(window, cx, &fe).is_empty());
        choose(window, cx, mono_approval::Choice::Run);
        let reused = answer.try_recv().unwrap().unwrap();
        assert_eq!(
            (reused["id"].as_str(), reused["reused"].as_bool()),
            (Some(fe.as_str()), Some(true))
        );
        assert_eq!(sent(window, cx, &fe), b"npm run web\r");
        // Another Mono's open of the same label is its own terminal.
        let other = call(
            window,
            cx,
            "m2",
            "terminals.open",
            json!({"label": "BE", "place": "dock", "reveal": false}),
        )
        .unwrap();
        assert_eq!(other["reused"], false);
    }

    /// A second local session of the project, in the sidebar order.
    fn extra_session(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> i64 {
        window
            .update(cx, |shell, _, _| {
                let session = shell
                    .store
                    .lock()
                    .unwrap()
                    .create("claude", PROJECT)
                    .unwrap();
                let id = session.id;
                shell.session_summaries.insert(id, session);
                shell.order.push(id);
                id
            })
            .unwrap()
    }

    fn stale_drag(from: usize, to: usize, tabs: usize) -> super::super::terminal_dock::DockTabDrag {
        super::super::terminal_dock::DockTabDrag {
            start: gpui::point(px(0.), px(0.)),
            from,
            to,
            origin: px(0.),
            offset: px(0.),
            active: true,
            bounds: (0..tabs)
                .map(|i| {
                    gpui::Bounds::new(
                        gpui::point(px(i as f32 * 80.), px(0.)),
                        gpui::size(px(80.), px(20.)),
                    )
                })
                .collect(),
        }
    }

    #[gpui::test]
    fn a_mono_closing_a_dock_terminal_cancels_a_tab_drag_and_a_tab_menu(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let user = user_terminal(window, cx);
        let be = dock(window, cx, "BE");
        let fe = dock(window, cx, "FE");
        let fe_t = terminal(window, cx, &fe);
        // The user holds the last tab in a drag; the Mono closes another one.
        window
            .update(cx, |shell, _, _| {
                shell.dock_drag = Some(stale_drag(2, 0, 3))
            })
            .unwrap();
        call(window, cx, "m1", "terminals.close", json!({"id": be})).unwrap();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.dock_drag.is_none(), "the drag was cancelled");
                shell.finish_dock_drag(cx);
                assert_eq!(shell.terminals[PROJECT], [user.clone(), fe_t.clone()]);
                // A drag that outlived the change anyway cannot index past the dock.
                shell.dock_drag = Some(stale_drag(2, 0, 3));
                shell.finish_dock_drag(cx);
                assert_eq!(shell.terminals[PROJECT], [user.clone(), fe_t.clone()]);
                assert_eq!(shell.workspace.docks[PROJECT].terminals.len(), 2);
            })
            .unwrap();
        // An open tab menu names its tab by position: closed when a Mono changes the dock.
        window
            .update(cx, |shell, w, cx| {
                shell.open_menu(
                    MenuTarget::TerminalTab(1),
                    Anchor::Point(gpui::point(px(10.), px(10.))),
                    w,
                    cx,
                );
                assert!(shell.menu.is_some());
            })
            .unwrap();
        call(window, cx, "m1", "terminals.close", json!({"id": fe})).unwrap();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.menu.is_none());
                assert_eq!(shell.terminals[PROJECT], std::slice::from_ref(&user));
            })
            .unwrap();
        // Moving a dock terminal into a pane (terminals.layout) cancels a drag too.
        let db = dock(window, cx, "DB");
        window
            .update(cx, |shell, _, _| {
                shell.dock_drag = Some(stale_drag(1, 0, 2))
            })
            .unwrap();
        layout(window, cx, "columns", &[&db]).unwrap();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.dock_drag.is_none());
                assert_eq!(shell.terminals[PROJECT], std::slice::from_ref(&user));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_mono_pane_never_blocks_deleting_the_session_it_borrows_and_moves_on(
        cx: &mut TestAppContext,
    ) {
        let (window, chat) = logs_setup(cx);
        let other = extra_session(window, cx);
        let id = open(
            window,
            cx,
            json!({"label": "BE", "place": "split", "reveal": false}),
        );
        let pane = pane_of(window, cx, &id);
        window
            .update(cx, |shell, _, _| {
                shell.workspace.terminal_panes.get_mut(&pane).unwrap().owner = other;
            })
            .unwrap();
        window
            .update(cx, |shell, w, cx| shell.delete_session(other, w, cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(!shell.session_summaries.contains_key(&other));
                let state = &shell.workspace.terminal_panes[&pane];
                assert_eq!(state.owner, chat);
                assert_eq!(state.mono.as_deref(), Some("m1"));
                assert!(shell.workspace.tab_of(pane).is_some());
            })
            .unwrap();
        // The last session of the project goes: the pane moves to the dock, still the same
        // terminal with its label and Mono.
        let shell_before = terminal(window, cx, &id);
        window
            .update(cx, |shell, w, cx| shell.delete_session(chat, w, cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(!shell.session_summaries.contains_key(&chat));
                assert!(shell.workspace.terminal_panes.is_empty());
                assert!(shell.workspace.tab_of(pane).is_none());
                let meta = shell.workspace.docks[PROJECT].mono_meta(0).unwrap();
                assert_eq!((meta.mono.as_str(), meta.label.as_str()), ("m1", "BE"));
                assert_eq!(
                    shell.terminals[PROJECT],
                    std::slice::from_ref(&shell_before)
                );
            })
            .unwrap();
        let list = rows(window, cx);
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0]["id"].as_str(), list[0]["place"].as_str()),
            (Some(id.as_str()), Some("dock"))
        );
    }

    #[gpui::test]
    fn a_users_terminal_pane_still_blocks_deleting_its_session_and_says_so(
        cx: &mut TestAppContext,
    ) {
        let (window, chat) = logs_setup(cx);
        let (report, answer) = async_channel::bounded(1);
        window
            .update(cx, |shell, w, cx| {
                let pane = shell.workspace.next_pane_id();
                shell
                    .workspace
                    .terminal_panes
                    .insert(pane, TerminalPane::new(chat, PROJECT.into()));
                shell.workspace.open(pane);
                shell.delete_session_with(
                    chat,
                    super::super::project_removal::DeleteOptions {
                        report: Some(report),
                        ..Default::default()
                    },
                    w,
                    cx,
                );
                assert!(shell.session_summaries.contains_key(&chat));
            })
            .unwrap();
        let error = answer
            .try_recv()
            .expect("the call was answered")
            .unwrap_err();
        assert!(error.contains("terminal panes"), "{error}");
    }

    #[gpui::test]
    fn a_pane_never_borrows_a_cloud_or_remote_session(cx: &mut TestAppContext) {
        let (window, chat) = logs_setup(cx);
        let cloud = extra_session(window, cx);
        let remote = extra_session(window, cx);
        window
            .update(cx, |shell, _, _| {
                shell.cloud_marks.insert(
                    cloud,
                    cloud::CloudMark {
                        local_project: PROJECT.into(),
                        codespace: "cs".into(),
                        kind: monocode_core::cloud_sessions::CloudBackendKind::Codespaces,
                    },
                );
                shell.session_summaries.get_mut(&remote).unwrap().cwd = "remote://host/p".into();
                shell.order.retain(|id| *id != chat);
                shell.order.extend([cloud, remote, chat]);
                shell.workspace.project_focus.insert(PROJECT.into(), cloud);
                // Both are listed first, yet neither is chosen.
                assert_eq!(shell.mono_pane_owner(PROJECT, None), Some(chat));
                assert_eq!(shell.mono_pane_owner(PROJECT, Some(chat)), None);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_second_split_joins_the_logs_tab_of_the_first_and_leaves_the_chat_alone(
        cx: &mut TestAppContext,
    ) {
        let (window, chat) = logs_setup(cx);
        let be = open(
            window,
            cx,
            json!({"label": "BE", "place": "split", "reveal": false}),
        );
        assert_eq!(rows(window, cx)[0]["place"], "tab");
        let fe = open(
            window,
            cx,
            json!({"label": "FE", "place": "split", "reveal": false}),
        );
        let (be_pane, fe_pane) = (pane_of(window, cx, &be), pane_of(window, cx, &fe));
        window
            .update(cx, |shell, _, _| {
                let w = &shell.workspace;
                assert_eq!(w.tab_of(be_pane), w.tab_of(fe_pane));
                assert_ne!(w.tab_of(be_pane), w.tab_of(chat));
                assert_eq!(w.layout(w.tab_of(chat).unwrap()).leaf_ids(), vec![chat]);
                assert_eq!(w.active, Some(chat));
            })
            .unwrap();
        assert!(rows(window, cx).iter().all(|r| r["place"] == "split"));
    }

    /// The call as `ask` makes it, but without `show_mono_approval`: what the call itself
    /// left behind (the `switched` flag) can be read before the shell root takes the keyboard.
    fn call_only(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        input: Value,
    ) -> Result<Value, String> {
        match window
            .update(cx, |shell, _, cx| {
                shell.mono_terminals_call("m1", PROJECT, "terminals.open", &input, now(), cx)
            })
            .unwrap()
        {
            MonoTerminalReply::Now(result) => result,
            MonoTerminalReply::Later(_) => panic!("waited for the user"),
        }
    }

    fn switched(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |shell, _, _| {
                std::mem::take(&mut shell.mono_approvals.switched)
            })
            .unwrap()
    }

    #[gpui::test]
    fn reusing_a_pane_in_another_tab_with_reveal_true_switches_and_keeps_the_keyboard_off_it(
        cx: &mut TestAppContext,
    ) {
        let (window, chat) = logs_setup(cx);
        let be = open(
            window,
            cx,
            json!({"label": "BE", "place": "split", "reveal": false}),
        );
        let pane = pane_of(window, cx, &be);
        let user = user_terminal(window, cx);
        window
            .update(cx, |_, w, cx| w.focus(&user.focus_handle(cx), cx))
            .unwrap();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.active_tab(), shell.workspace.tab_of(chat));
            })
            .unwrap();
        // Reuse without an explicit reveal never navigates.
        let quiet = ask(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "split"}),
        );
        let MonoTerminalReply::Now(Ok(quiet)) = quiet else {
            panic!("waited");
        };
        assert_eq!(
            (quiet["reused"].as_bool(), quiet["revealed"].as_bool()),
            (Some(true), Some(false))
        );
        assert!(!switched(window, cx));
        // An explicit reveal selects the tab, flags it, and the shell root takes the keyboard.
        let done = call_only(
            window,
            cx,
            json!({"label": "BE", "place": "split", "reveal": true}),
        )
        .unwrap();
        assert_eq!(done["reused"], true);
        assert!(done.get("revealed").is_none(), "{done}");
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.active_tab(), shell.workspace.tab_of(pane));
                assert!(shell.mono_approvals.switched);
            })
            .unwrap();
        window
            .update(cx, |shell, w, cx| shell.show_mono_approval(w, cx))
            .unwrap();
        window
            .update(cx, |shell, w, cx| {
                let terminal = shell.terminal_pane_views[&pane].focus_handle(cx);
                assert!(w.focused(cx) != Some(terminal));
                assert_eq!(w.focused(cx), Some(shell.focus.clone()));
                assert!(!shell.mono_approvals.switched);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_new_pane_in_another_tab_flags_the_switch_too(cx: &mut TestAppContext) {
        let (window, chat) = logs_setup(cx);
        let done = call_only(
            window,
            cx,
            json!({"label": "NEW", "place": "split", "reveal": true}),
        )
        .unwrap();
        assert_eq!(done["reused"], false);
        window
            .update(cx, |shell, _, _| {
                assert_ne!(shell.workspace.active_tab(), shell.workspace.tab_of(chat));
            })
            .unwrap();
        assert!(switched(window, cx), "the visible tab changed");
        // Staying in the tab that is already visible is no switch.
        let joined = call_only(
            window,
            cx,
            json!({"label": "TWO", "place": "split", "reveal": true}),
        )
        .unwrap();
        assert_eq!(joined["reused"], false);
        assert!(!switched(window, cx), "the Logs tab was already visible");
    }

    #[gpui::test]
    fn a_surface_that_stays_on_screen_keeps_the_keyboard_after_a_switch(cx: &mut TestAppContext) {
        let (window, _chat) = logs_setup(cx);
        let be = open(
            window,
            cx,
            json!({"label": "BE", "place": "split", "reveal": false}),
        );
        let pane = pane_of(window, cx, &be);
        // The right side panel is open and has the keyboard.
        let side = window
            .update(cx, |shell, w, cx| {
                shell.set_right_panel_open(true, w, cx);
                shell.right_panel.as_ref().unwrap().focus_handle(cx)
            })
            .unwrap();
        cx.run_until_parked();
        window.update(cx, |_, w, cx| w.focus(&side, cx)).unwrap();
        cx.run_until_parked();
        // New pane and reused pane follow the same rule.
        for label in ["BE", "FRESH"] {
            let done = ask(
                window,
                cx,
                "m1",
                "terminals.open",
                json!({"label": label, "place": "split", "reveal": true}),
            );
            let MonoTerminalReply::Now(Ok(done)) = done else {
                panic!("waited");
            };
            assert_eq!(done["reused"].as_bool(), Some(label == "BE"));
            window
                .update(cx, |shell, w, cx| {
                    assert_eq!(w.focused(cx), Some(side.clone()), "{label}");
                    assert!(shell.workspace.active_tab().is_some());
                })
                .unwrap();
        }
        let _ = pane;
    }

    #[gpui::test]
    fn approving_gives_the_keyboard_back_to_where_it_was(cx: &mut TestAppContext) {
        let (window, chat) = logs_setup(cx);
        let user = user_terminal(window, cx);
        let focus = |cx: &mut TestAppContext| window.update(cx, |_, w, cx| w.focused(cx)).unwrap();
        window
            .update(cx, |_, w, cx| w.focus(&user.focus_handle(cx), cx))
            .unwrap();
        let before = focus(cx);
        assert!(before.is_some());
        for choice in [
            mono_approval::Choice::Run,
            mono_approval::Choice::Deny,
            mono_approval::Choice::Expired,
        ] {
            let answer = wait(
                window,
                cx,
                "m1",
                "terminals.open",
                json!({"label": "FE", "command": "ls", "place": "split", "reveal": true}),
            );
            assert!(focus(cx) != before, "the dialog holds the keyboard");
            choose(window, cx, choice.clone());
            let _ = answer.try_recv();
            if choice == mono_approval::Choice::Run {
                // The approved call selected the Mono's tab, which hides what had the
                // keyboard: the shell root takes it, never the terminal.
                let root = window
                    .update(cx, |shell, _, _| shell.focus.clone())
                    .unwrap();
                assert!(focus(cx) == Some(root), "{choice:?}");
                window
                    .update(cx, |_, w, cx| w.focus(&user.focus_handle(cx), cx))
                    .unwrap();
            } else {
                assert!(focus(cx) == before, "{choice:?}");
            }
        }
        // Revealing selected the Mono's tab but the keyboard did not follow.
        window
            .update(cx, |shell, _, _| {
                assert_ne!(shell.workspace.active, Some(chat));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_pane_dragged_out_of_the_logs_tab_is_not_a_logs_tab_any_more(cx: &mut TestAppContext) {
        let (window, _chat) = logs_setup(cx);
        let be = dock(window, cx, "BE");
        let fe = dock(window, cx, "FE");
        layout(window, cx, "columns", &[&be, &fe]).unwrap();
        let (be_pane, fe_pane) = (pane_of(window, cx, &be), pane_of(window, cx, &fe));
        // The user pulls FE onto the title strip.
        window
            .update(cx, |shell, _, _| {
                let end = shell.workspace.tabs.len();
                shell.workspace.detach(fe_pane, end);
                let w = &shell.workspace;
                assert_ne!(w.tab_of(fe_pane), w.tab_of(be_pane));
                assert_eq!(w.terminal_panes[&fe_pane].logs, None);
                assert_eq!(w.logs_title(w.tab_of(fe_pane).unwrap()), None);
                assert_eq!(
                    w.logs_title(w.tab_of(be_pane).unwrap()),
                    Some("Logs monocode")
                );
            })
            .unwrap();
        // A new split joins BE's Logs tab, not the tab FE was pulled into.
        let qa = open(
            window,
            cx,
            json!({"label": "QA", "place": "split", "reveal": false}),
        );
        let qa_pane = pane_of(window, cx, &qa);
        window
            .update(cx, |shell, _, _| {
                let w = &shell.workspace;
                assert_eq!(w.tab_of(qa_pane), w.tab_of(be_pane));
                assert_ne!(w.tab_of(qa_pane), w.tab_of(fe_pane));
            })
            .unwrap();
    }

    #[gpui::test]
    fn layout_leaves_the_active_pane_none_when_nothing_was_active(cx: &mut TestAppContext) {
        let (window, _chat) = logs_setup(cx);
        let be = dock(window, cx, "BE");
        let fe = dock(window, cx, "FE");
        window
            .update(cx, |shell, _, _| shell.workspace.active = None)
            .unwrap();
        layout(window, cx, "columns", &[&be, &fe]).unwrap();
        window
            .update(cx, |shell, _, _| assert_eq!(shell.workspace.active, None))
            .unwrap();
    }

    #[gpui::test]
    fn reveal_says_when_it_could_not_show_the_terminal(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        dock(window, cx, "BE");
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.reveal_mono_terminal(PROJECT, Spot::Dock(0), false, None, cx));
                // Not the project in view: nothing to show.
                assert!(!shell.reveal_mono_terminal("/elsewhere", Spot::Dock(0), true, None, cx));
            })
            .unwrap();
        // A background open reports nothing odd; only a reveal that failed says so.
        let ok = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "FE", "place": "dock", "reveal": true}),
        )
        .unwrap();
        assert!(ok.get("revealed").is_none());
    }

    #[gpui::test]
    fn a_project_spelled_with_a_trailing_slash_is_the_same_project(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let spelled = format!("{PROJECT}/");
        let opened = window
            .update(cx, |shell, _, cx| {
                shell.mono_terminals_call(
                    "m1",
                    &spelled,
                    "terminals.open",
                    &json!({"label": "BE", "place": "dock", "reveal": false}),
                    now(),
                    cx,
                )
            })
            .unwrap();
        let MonoTerminalReply::Now(Ok(opened)) = opened else {
            panic!("the open failed");
        };
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.terminals.keys().collect::<Vec<_>>(), [PROJECT]);
                assert_eq!(shell.workspace.docks.keys().collect::<Vec<_>>(), [PROJECT]);
            })
            .unwrap();
        // Listed under the plain spelling, and reused through the other one.
        let list = rows(window, cx);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["id"], opened["id"]);
        let again = window
            .update(cx, |shell, _, cx| {
                shell.mono_terminals_call(
                    "m1",
                    &spelled,
                    "terminals.open",
                    &json!({"label": "BE", "place": "dock", "reveal": false}),
                    now(),
                    cx,
                )
            })
            .unwrap();
        let MonoTerminalReply::Now(Ok(again)) = again else {
            panic!("the second open failed");
        };
        assert_eq!(
            (again["reused"].clone(), again["id"].clone()),
            (json!(true), opened["id"].clone())
        );
    }

    #[gpui::test]
    fn let_mono_read_is_offered_for_plain_user_shells_only(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let user = user_terminal(window, cx);
        let mono = dock(window, cx, "BE");
        let mono_t = terminal(window, cx, &mono);
        window
            .update(cx, |shell, _, cx| {
                assert!(Shell::offers_mono_read(user.read(cx)));
                assert!(!Shell::offers_mono_read(mono_t.read(cx)));
                // Close, Close Others and a separator for a Mono's tab; one more item for the
                // user's.
                assert_eq!(shell.terminal_tab_menu(0, cx).len(), 4);
                assert_eq!(shell.terminal_tab_menu(1, cx).len(), 3);
                user.update(cx, |t, _| t.pretend_launched());
                assert!(!Shell::offers_mono_read(user.read(cx)));
                assert_eq!(shell.terminal_tab_menu(0, cx).len(), 3);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_restart_listing_spawns_only_the_shells_up_to_the_last_mono_terminal(
        cx: &mut TestAppContext,
    ) {
        // A project that is not in view: its dock was saved with a Mono's tab and two of
        // the user's.
        let away = "/work/another-project";
        let (window, _chat) = logs_setup(cx);
        window
            .update(cx, |shell, _, _| {
                let dock = shell.workspace.docks.entry(away.into()).or_default();
                for _ in 0..3 {
                    dock.add(away.into());
                }
                dock.set_mono_meta(
                    0,
                    Some(MonoTerminalMeta {
                        mono: "m1".into(),
                        label: "BE".into(),
                        run: None,
                    }),
                );
            })
            .unwrap();
        let json = saved(window, cx);
        let window = restarted(cx, json);
        add_monos(window, cx, &["m1"]);
        let reply = window
            .update(cx, |shell, _, cx| {
                shell.mono_manager().update(cx, |monos, _| {
                    monos
                        .roster
                        .get_mut("m1")
                        .unwrap()
                        .projects
                        .push(away.into())
                });
                shell.mono_terminals_call("m1", away, "terminals.list", &json!({}), now(), cx)
            })
            .unwrap();
        let MonoTerminalReply::Now(Ok(list)) = reply else {
            panic!("the list failed");
        };
        assert_eq!(list.as_array().unwrap().len(), 1, "{list:?}");
        assert_eq!(list[0]["label"], "BE");
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.docks[away].terminals.len(), 3);
                assert_eq!(
                    shell.terminals[away].len(),
                    1,
                    "the user's tabs stay unspawned"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn run_again_shows_and_runs_the_actions_current_command(cx: &mut TestAppContext) {
        let json = before_restart(cx);
        let window = restarted(cx, json);
        add_monos(window, cx, &["m1"]);
        cx.update(|cx| {
            bind_keys(cx);
            crate::terminal::bind(cx);
            cx.set_reduce_motion(true);
        });
        let list = rows(window, cx);
        let be = list.iter().find(|r| r["label"] == "BE").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        // The saved action was edited while the app was closed.
        window
            .update(cx, |shell, _, _| {
                shell.workspace.actions.insert(
                    PROJECT.into(),
                    vec![ProjectAction {
                        id: "a1".into(),
                        name: "Dev".into(),
                        command: "npm run dev -- --port 4000".into(),
                        ..Default::default()
                    }],
                );
            })
            .unwrap();
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let button = visual.debug_bounds("terminal-run-again").unwrap();
        visual.simulate_click(button.center(), gpui::Modifiers::default());
        visual.run_until_parked();
        // The press only brought the bar up to date: what it shows is what runs next.
        assert!(sent(window, cx, &be).is_empty());
        let t = terminal(window, cx, &be);
        window
            .update(cx, |_, _, cx| {
                assert_eq!(
                    t.read(cx).run_again.as_ref().unwrap().text,
                    "npm run dev -- --port 4000"
                );
            })
            .unwrap();
        let button = visual.debug_bounds("terminal-run-again").unwrap();
        visual.simulate_click(button.center(), gpui::Modifiers::default());
        visual.run_until_parked();
        assert_eq!(sent(window, cx, &be), b"npm run dev -- --port 4000\r");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mono-terminals-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_cwd_allows_the_project_and_its_worktrees_only() {
        let root = scratch("cwd");
        let project = root.join("app");
        let tree = root.join("app-worktrees").join("feat");
        let outside = root.join("elsewhere");
        for dir in [project.join("src"), tree.clone(), outside.clone()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let project_text = project.to_string_lossy().into_owned();
        let trees = || vec![tree.to_string_lossy().into_owned()];
        let none = || -> Vec<String> { panic!("worktrees asked for nothing") };
        assert_eq!(
            resolve_cwd(&project_text, None, none).unwrap(),
            project_text
        );
        let src = resolve_cwd(&project_text, Some("src"), none).unwrap();
        assert!(src.ends_with("src"));
        assert!(
            resolve_cwd(
                &project_text,
                Some(&project.join("src").to_string_lossy()),
                none
            )
            .is_ok()
        );
        assert!(resolve_cwd(&project_text, Some(&tree.to_string_lossy()), trees).is_ok());
        for escape in [
            outside.to_string_lossy().into_owned(),
            format!("{project_text}/../elsewhere"),
            "../elsewhere".into(),
            "nope".into(),
        ] {
            assert!(
                resolve_cwd(&project_text, Some(&escape), trees).is_err(),
                "{escape}"
            );
        }
        // A sibling that merely shares the prefix is not inside.
        let sibling = root.join("app-evil");
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(resolve_cwd(&project_text, Some(&sibling.to_string_lossy()), Vec::new).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, project.join("link")).unwrap();
            assert!(resolve_cwd(&project_text, Some("link"), Vec::new).is_err());
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saved_terminal_panes_from_before_labels_still_load() {
        let old: TerminalPane = serde_json::from_str(r#"{"owner":3,"cwd":"/w"}"#).unwrap();
        assert_eq!(old, TerminalPane::new(3, "/w".into()));
        // The new fields stay out of the file until a Mono sets them.
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"{"owner":3,"cwd":"/w"}"#
        );
        let mut named = old;
        named.label = Some("BE".into());
        let back: TerminalPane =
            serde_json::from_str(&serde_json::to_string(&named).unwrap()).unwrap();
        assert_eq!(back, named);
    }
}
