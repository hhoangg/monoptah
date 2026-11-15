//! Terminals a Mono opens and drives through `terminals.*` (see docs/agent-access.md).
//!
//! Terminals stay ordinary dock tabs or terminal panes. This module only adds a registry of
//! opaque ids (`t-<n>`) over them, the safety rules, and the calls themselves. The registry
//! never stores a position: a dock tab can be reordered or closed by the user at any time, so
//! every call finds the terminal again by entity identity (`terminal_spot`) and forgets entries
//! whose terminal is gone.
//!
//! A free `command` (`terminals.open`) and typed `text` (`terminals.send`) wait for the user:
//! the call validates everything, then either runs at once (the Mono's remembered exact
//! string, see `mono_approval.rs`) or returns a `Step::Ask` that the shell queues as a dialog.
//! Approving runs the very same function again with `approved` set, so every guard (terminal
//! still there, owned by the Mono, cwd inside the project) is checked a second time right
//! before anything is written. A saved Project Action never asks: the user wrote it.
use super::*;
use crate::{
    pane_layout::Edge,
    terminal::{MonoKey, Terminal},
    workspace::TerminalPane,
};
use serde_json::{Value, json};
use std::{
    path::{Component, Path},
    time::Duration,
};

/// The most bytes of a free `command` or typed `text`.
const INPUT_MAX: usize = monocode_core::mono::api::TERMINAL_INPUT_MAX;
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
    pub kind: &'static str,
    /// The exact string, as the Mono sent it.
    pub text: String,
    /// The terminal's name: the label the Mono asked for, else the terminal's current title.
    pub label: String,
    pub cwd: String,
    /// A key the call presses after the text.
    pub then_key: Option<String>,
    /// The call to run again, once approved.
    pub input: Value,
    pub action: &'static str,
}

enum Step {
    Done(Value),
    Ask(Request),
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

#[derive(Default)]
pub(super) struct Registry {
    next: u64,
    entries: Vec<Entry>,
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
    if label.is_empty() || label.chars().any(char::is_control) {
        return Err("Provide a label: one line of text".into());
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
/// bytes, and no control character except line breaks and tabs. ESC, Enter (`\r`), Ctrl-C and
/// the rest are keys, never text, so a Mono cannot type an escape sequence.
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
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
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
        cx: &mut Context<Self>,
    ) -> MonoTerminalReply {
        match self.mono_terminals_step(mono, project, action, input, false, cx) {
            Ok(Step::Done(value)) => MonoTerminalReply::Now(Ok(value)),
            Ok(Step::Ask(request)) => self.queue_mono_approval(mono, project, request, cx),
            Err(error) => MonoTerminalReply::Now(Err(error)),
        }
    }

    /// Runs a call that was approved: the same checks as the call itself, now allowed to write.
    pub(super) fn mono_terminals_approved(
        &mut self,
        mono: &str,
        project: &str,
        action: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        match self.mono_terminals_step(mono, project, action, input, true, cx)? {
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
        approved: bool,
        cx: &mut Context<Self>,
    ) -> Result<Step, String> {
        if project.starts_with("remote://") {
            return Err("Terminals are not available on remote projects".into());
        }
        self.prune_mono_terminals();
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
            _ => Err("Unknown terminals action".into()),
        }
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
                    self.find_mono_terminal(mono, project, id)?;
                }
                Ok(())
            }
        }
    }

    /// Whether the user let this Mono run exactly `text` as `kind` without asking.
    fn mono_allows(&self, mono: &str, kind: &str, text: &str, cx: &App) -> bool {
        self.monos
            .as_ref()
            .is_some_and(|monos| monos.read(cx).terminal_allowed(mono, kind, text))
    }

    /// The project a terminal pane belongs to: its owner session's.
    fn terminal_pane_project(&self, pane: i64) -> Option<String> {
        let owner = self.workspace.terminal_panes.get(&pane)?.owner;
        let summary = self.session_summaries.get(&owner)?;
        Some(self.project_of(owner, &summary.cwd).to_owned())
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
        let id = format!("t-{}", registry.next);
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
            let place = match spot {
                Spot::Dock(_) => "dock",
                Spot::Pane(pane) => {
                    let split = self
                        .workspace
                        .tab_of(pane)
                        .is_some_and(|tab| self.workspace.layout(tab).is_split());
                    if split { "split" } else { "tab" }
                }
            };
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

    /// A saved Project Action reaches a shell here, with no question: the user wrote it. A free
    /// command goes through `Terminal::send_text` instead, and only once approved.
    fn run_in_terminal(&mut self, terminal: &Entity<Terminal>, command: &str, cx: &mut App) {
        terminal.update(cx, |t, cx| t.run(command, cx));
    }

    fn mono_open_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        approved: bool,
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
            monocode_core::git::worktrees(Path::new(project))
                .map(|trees| trees.into_iter().map(|t| t.path).collect())
                .unwrap_or_default()
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
        // Visible by default only where the user is looking.
        let reveal = input.get("reveal").and_then(Value::as_bool).unwrap_or(
            monocode_core::files::path_key(project)
                == monocode_core::files::path_key(&self.workspace.project),
        );
        let beside = match input.get("beside").and_then(Value::as_str) {
            None => None,
            Some(id) => Some(self.find_mono_terminal(mono, project, id)?),
        };
        if let Some(command) = &command
            && !approved
            && !self.mono_allows(mono, "command", command, cx)
        {
            return Ok(Step::Ask(Request {
                kind: "command",
                text: command.clone(),
                label,
                cwd,
                then_key: None,
                input: input.clone(),
                action: "terminals.open",
            }));
        }

        // An idle terminal with this label is reused, like an action's own terminal.
        let reusable = self.mono_registry.entries.iter().find_map(|entry| {
            let terminal = entry.terminal.upgrade()?;
            let t = terminal.read(cx);
            let same = entry.origin == Origin::Mono
                && entry.mono.as_deref() == Some(mono)
                && monocode_core::files::path_key(&entry.project)
                    == monocode_core::files::path_key(project)
                && t.label.as_deref() == Some(label.as_str())
                && t.foreground.is_none()
                && !t.exited
                && input.get("cwd").is_none_or(|_| {
                    monocode_core::files::path_key(&t.cwd) == monocode_core::files::path_key(&cwd)
                });
            same.then(|| (entry.id.clone(), terminal.clone()))
        });
        if let Some((id, terminal)) = reusable
            && let Some(spot) = self.terminal_spot(project, &terminal)
        {
            if let Some(action) = &action {
                self.run_in_terminal(&terminal, &action.command, cx);
                if let Some(entry) = self.mono_registry.entries.iter_mut().find(|e| e.id == id) {
                    entry.action = Some(action.id.clone());
                }
            }
            if let Some(command) = &command {
                terminal.update(cx, |t, cx| t.send_text(command, true, cx));
            }
            if reveal {
                self.reveal_mono_terminal(project, spot, cx);
            }
            return Ok(Step::Done(
                json!({"id": id, "reused": true, "place": spot_name(spot)}),
            ));
        }

        let in_panel = self.terminal_in_panel(cx);
        let split = match place {
            Some(place) => place == "split",
            None => self.workspace.preferences.terminal_placement() == "title" && !in_panel,
        };
        let mut opened = None;
        if split {
            let target = self.mono_split_target(mono, project, beside.as_ref())?;
            opened =
                self.open_mono_terminal_pane(mono, project, &label, &cwd, target, edge, reveal, cx);
        } else if let Some(beside) = &beside
            && matches!(beside.spot, Spot::Pane(_))
        {
            // Beside a pane means a pane, whatever the user's default says.
            let target = self.mono_split_target(mono, project, Some(beside))?;
            opened =
                self.open_mono_terminal_pane(mono, project, &label, &cwd, target, edge, reveal, cx);
        }
        let (terminal, spot) = match opened {
            Some(opened) => opened,
            None => self.open_mono_dock_terminal(project, &label, &cwd, reveal, in_panel, cx),
        };
        let id = self.registry_id(&terminal, Origin::Mono, Some(mono), project);
        if let Some(action) = &action {
            self.run_in_terminal(&terminal, &action.command, cx);
            if let Some(entry) = self.mono_registry.entries.iter_mut().find(|e| e.id == id) {
                entry.action = Some(action.id.clone());
            }
        }
        if let Some(command) = &command {
            terminal.update(cx, |t, cx| t.send_text(command, true, cx));
        }
        if reveal {
            self.reveal_mono_terminal(project, spot, cx);
        }
        cx.notify();
        Ok(Step::Done(
            json!({"id": id, "reused": false, "place": spot_name(spot)}),
        ))
    }

    /// The pane a new split terminal goes beside: `beside`, else the Mono's latest terminal
    /// pane of the project, else the active pane when it belongs to the project. `None` opens
    /// a title tab of its own.
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
        let own = self.mono_registry.entries.iter().rev().find_map(|entry| {
            let terminal = entry.terminal.upgrade()?;
            let Spot::Pane(pane) = self.terminal_spot(project, &terminal)? else {
                return None;
            };
            (entry.origin == Origin::Mono
                && entry.mono.as_deref() == Some(mono)
                && self.workspace.tab_of(pane).is_some())
            .then_some(pane)
        });
        Ok(own.or_else(|| {
            self.workspace
                .active
                .filter(|id| self.workspace.tab_of(*id).is_some() && in_project(*id))
        }))
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
        cx: &mut Context<Self>,
    ) -> Option<(Entity<Terminal>, Spot)> {
        let owner = self.mono_pane_owner(project)?;
        let terminal = self.spawn_terminal(cwd.to_owned(), cx);
        terminal.update(cx, |t, _| t.label = Some(label.to_owned()));
        let pane = self.workspace.next_pane_id();
        let mut state = TerminalPane::new(owner, cwd.to_owned());
        state.mono = Some(mono.to_owned());
        state.label = Some(label.to_owned());
        self.workspace.terminal_panes.insert(pane, state);
        self.terminal_pane_views.insert(pane, terminal.clone());
        let previous = self.workspace.active;
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
        if !reveal {
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
    fn mono_pane_owner(&self, project: &str) -> Option<i64> {
        let key = monocode_core::files::path_key(project);
        let known = |id: i64| {
            self.session_summaries
                .get(&id)
                .filter(|s| monocode_core::files::path_key(self.project_of(id, &s.cwd)) == key)
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

    fn open_mono_dock_terminal(
        &mut self,
        project: &str,
        label: &str,
        cwd: &str,
        reveal: bool,
        in_panel: bool,
        cx: &mut Context<Self>,
    ) -> (Entity<Terminal>, Spot) {
        self.ensure_terminals(project, cx);
        let viewport = self.viewport();
        let dock = self.workspace.docks.entry(project.to_owned()).or_default();
        let before = (!dock.terminals.is_empty()).then_some((dock.active, dock.open));
        dock.size = dock.side.clamp(dock.size, viewport);
        let ix = dock.add(cwd.to_owned());
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
        let terminal = self.spawn_terminal(cwd.to_owned(), cx);
        terminal.update(cx, |t, _| t.label = Some(label.to_owned()));
        self.terminals
            .entry(project.to_owned())
            .or_default()
            .push(terminal.clone());
        self.persist(cx);
        (terminal, Spot::Dock(ix))
    }

    /// Shows a terminal without taking the keyboard or leaving a Mono's view. Only where the
    /// user is looking: another project's dock or tabs are not in view.
    fn reveal_mono_terminal(&mut self, project: &str, spot: Spot, cx: &mut Context<Self>) {
        if monocode_core::files::path_key(project)
            != monocode_core::files::path_key(&self.workspace.project)
        {
            return;
        }
        match spot {
            Spot::Dock(ix) => {
                let in_panel = self.terminal_in_panel(cx);
                let Some(dock) = self.workspace.docks.get_mut(project) else {
                    return;
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
            }
            Spot::Pane(pane) => self.reveal_terminal_pane(pane, cx),
        }
    }

    fn mono_send_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        approved: bool,
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
        let label = {
            let t = found.terminal.read(cx);
            if t.exited {
                return Err("The terminal has exited".into());
            }
            if t.agent().is_some() {
                return Err("This terminal runs an agent CLI; Mono never types into those".into());
            }
            (t.display_title().to_owned(), t.cwd.clone())
        };
        if let Some(text) = &text
            && !approved
            && !self.mono_allows(mono, "text", text, cx)
        {
            return Ok(Step::Ask(Request {
                kind: "text",
                text: text.clone(),
                label: label.0,
                cwd: label.1,
                then_key: key.map(|(name, _)| name.to_owned()),
                input: input.clone(),
                action: "terminals.send",
            }));
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
        let (text, older) = t.recent_text(lines);
        let (text, capped) = cap_tail(monocode_core::mono::memory::redact(&text), READ_BYTES_MAX);
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
        found
            .terminal
            .update(cx, |t, cx| t.send_key(MonoKey::CtrlC, cx));
        let weak = found.terminal.downgrade();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(STOP_GRACE).await;
            let _ = weak.update(cx, |t, cx| {
                if t.foreground.is_some() && !t.exited {
                    t.stop_foreground(cx);
                }
            });
        })
        .detach();
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

    /// The terminal pane's "Let Mono read" switch, for its title tab or pane.
    pub(super) fn toggle_mono_read_pane(&mut self, pane: i64, cx: &mut Context<Self>) {
        if let Some(view) = self.terminal_pane_views.get(&pane) {
            view.update(cx, |t, cx| {
                t.mono_read = !t.mono_read;
                cx.notify();
            });
        }
    }

    pub(super) fn toggle_mono_read_dock(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(view) = self
            .terminals
            .get(&self.workspace.project)
            .and_then(|t| t.get(ix))
        {
            view.update(cx, |t, cx| {
                t.mono_read = !t.mono_read;
                cx.notify();
            });
        }
    }

    pub(super) fn mono_read_item(on: bool) -> MenuItem {
        MenuAction::new(
            "mono-read",
            if on {
                "Stop letting Mono read"
            } else {
                "Let Mono read"
            },
        )
        .into()
    }
}

fn spot_name(spot: Spot) -> &'static str {
    match spot {
        Spot::Dock(_) => "dock",
        Spot::Pane(_) => "split",
    }
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

    /// The call as the host makes it: with a window, then the dialog is put in front.
    fn ask(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        mono: &str,
        action: &str,
        input: Value,
    ) -> MonoTerminalReply {
        window
            .update(cx, |shell, w, cx| {
                let reply = shell.mono_terminals_call(mono, PROJECT, action, &input, cx);
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

    #[gpui::test]
    fn ids_follow_the_terminal_through_dock_reorders_and_user_closes(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let (be, fe) = (dock(window, cx, "BE"), dock(window, cx, "FE"));
        assert_eq!((be.as_str(), fe.as_str()), ("t-1", "t-2"));
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
            json!({"id": be, "key": "enter"}),
        )
        .unwrap();
        assert_eq!(sent(window, cx, &be), b"\r");
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
            json!({"id": be, "key": "enter"}),
        );
        assert!(gone.unwrap_err().contains("Unknown terminal t-1"));
        let list = rows(window, cx);
        assert_eq!(list.len(), 1);
        assert_eq!(
            (&list[0]["id"], &list[0]["label"]),
            (&json!("t-2"), &json!("FE"))
        );
        // Ids are not handed out twice.
        assert_eq!(dock(window, cx, "BE"), "t-3");
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
        assert_eq!(be["place"], "split");
        let fe = open(
            window,
            cx,
            json!({"label": "FE", "beside": be["id"], "direction": "down", "reveal": false}),
        );
        window
            .update(cx, |shell, _, cx| {
                let tab = shell.workspace.tab_of(chat).unwrap();
                assert_eq!(shell.workspace.layout(tab).leaf_ids().len(), 3);
                assert_eq!(shell.workspace.terminal_panes.len(), 2);
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
        assert_eq!(opened["place"], "split");
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
        // Pane: its tab is selected, nothing else moves.
        window
            .update(cx, |shell, _, _| {
                shell.workspace.preferences.set_terminal_placement("title")
            })
            .unwrap();
        let pane = open(
            window,
            cx,
            json!({"label": "FE", "place": "split", "reveal": true}),
        );
        window
            .update(cx, |shell, _, _| {
                let Spot::Pane(p) = shell.find_mono_terminal("m1", PROJECT, &pane).unwrap().spot
                else {
                    panic!("not a pane");
                };
                assert_eq!(shell.workspace.active, Some(p));
            })
            .unwrap();
        assert_eq!(snapshot(cx), before);
        let _ = hidden;
    }

    #[gpui::test]
    fn only_the_known_keys_reach_the_terminal(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let mut want = Vec::new();
        for (key, bytes) in [
            ("ctrl-c", &b"\x03"[..]),
            ("enter", b"\r"),
            ("up", b"\x1b[A"),
            ("down", b"\x1b[B"),
            ("q", b"q"),
            ("tab", b"\t"),
            ("esc", b"\x1b"),
        ] {
            call(
                window,
                cx,
                "m1",
                "terminals.send",
                json!({"id": id, "key": key}),
            )
            .unwrap();
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
    fn a_terminal_running_an_agent_cli_gets_no_keys(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
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
            json!({"id": id, "key": "enter"}),
        );
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
        // The tab menu's switch.
        window
            .update(cx, |shell, w, cx| {
                shell.run_terminal_tab_menu(0, "mono-read", w, cx)
            })
            .unwrap();
        assert!(call(window, cx, "m1", "terminals.read", json!({"id": id})).is_ok());
        window
            .update(cx, |shell, w, cx| {
                shell.run_terminal_tab_menu(0, "mono-read", w, cx)
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
                    cx,
                )
            })
            .unwrap();
        let MonoTerminalReply::Now(remote) = remote else {
            panic!("a refusal is immediate");
        };
        assert!(remote.unwrap_err().contains("remote"));
        assert!(call(window, cx, "m1", "terminals.read", json!({"id": "t-99"})).is_err());
        assert!(call(window, cx, "m1", "terminals.read", json!({})).is_err());
    }

    #[gpui::test]
    fn a_cwd_outside_the_project_is_refused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        for cwd in ["/tmp", "../etc", "sub/../../x", ""] {
            let err = call(
                window,
                cx,
                "m1",
                "terminals.open",
                json!({"label": "BE", "cwd": cwd}),
            );
            assert!(err.is_err(), "{cwd}");
        }
        assert!(
            call(
                window,
                cx,
                "m1",
                "terminals.open",
                json!({"label": "BE", "cwd": 3})
            )
            .is_err()
        );
        assert!(rows(window, cx).is_empty());
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

    #[gpui::test]
    fn enter_denies_until_the_focus_is_moved_to_run_and_the_guard_is_over(cx: &mut TestAppContext) {
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

        // Tab and an arrow reach Run, but a keystroke in the first moments approves nothing.
        let second = wait(window, cx, "m1", "terminals.open", open_command("BE", "ls"));
        visual.run_until_parked();
        visual.simulate_keystrokes("tab right");
        visual.run_until_parked();
        assert_eq!(
            window
                .update(cx, |s, _, _| s.mono_approvals.queue[0].selected)
                .unwrap(),
            2,
            "Tab and an arrow moved the focus to Run"
        );
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(waiting(window, cx), 1, "the guard held");
        assert!(second.try_recv().is_err());
        assert_eq!(all_terminals(window, cx), 0);
        guard_over(cx);
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        let id = second.try_recv().unwrap().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(sent(window, cx, &id), b"ls\r");
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
                assert_eq!(m.terminal_allow, ["command:npm test"]);
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
        for near in [
            "npm test ",
            " npm test",
            "npm  test",
            "npm Test",
            "npm tes",
            "npm test\n",
        ] {
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
            // Line breaks and tabs are text.
            let multi = wait(window, cx, "m1", action, build("one\ntwo\tthree"));
            choose(window, cx, mono_approval::Choice::Deny);
            assert_eq!(multi.try_recv().unwrap(), declined());
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
    fn multi_line_text_is_a_bracketed_paste_only_when_the_program_asked_for_one(
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
        // Off: each line ends in Enter, as a paste into a plain terminal does.
        run(cx, json!({"id": id, "text": "a\nb"}));
        assert_eq!(sent(window, cx, &id), b"a\rb");
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| t.fake_output("\x1b[?2004h", cx))
            })
            .unwrap();
        run(cx, json!({"id": id, "text": "c\nd"}));
        assert_eq!(sent(window, cx, &id), b"a\rb\x1b[200~c\nd\x1b[201~");
        // One line stays bare in both modes, and a key can follow in the same call.
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
    fn pending_approvals_queue_one_dialog_at_a_time_and_answer_in_order(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        add_monos(window, cx, &["m1", "m2"]);
        let first = wait(window, cx, "m1", "terminals.open", open_command("A", "one"));
        let second = wait(window, cx, "m2", "terminals.open", open_command("B", "two"));
        let third = wait(
            window,
            cx,
            "m1",
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
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
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
            "m1",
            "terminals.send",
            json!({"id": id, "text": "pwd"}),
        );
        window
            .update(cx, |shell, w, cx| shell.remove_terminal(PROJECT, 0, w, cx))
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
        add_monos(window, cx, &["m1"]);
        let id = dock(window, cx, "BE");
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
            "m1",
            "terminals.send",
            json!({"id": id, "text": "pwd"}),
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
