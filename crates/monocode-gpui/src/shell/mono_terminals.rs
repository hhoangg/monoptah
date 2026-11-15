//! Terminals a Mono opens and drives through `terminals.*` (see docs/agent-access.md).
//!
//! Terminals stay ordinary dock tabs or terminal panes. This module only adds a registry of
//! opaque ids (`t-<launch>-<n>`, the first part random per launch so a receipt from an earlier run cannot name a new terminal) over them, the scope rule, and the calls themselves. The registry
//! never stores a position: a dock tab can be reordered or closed by the user at any time, so
//! every call finds the terminal again by entity identity (`terminal_spot`) and forgets entries
//! whose terminal is gone.
//!
//! Input is free and has no approval: a `command` (`terminals.open`), typed `text` and every
//! key of `MonoKey` (`terminals.send`) are written at once. The one rule is scope: the
//! terminal's folder must be the project or inside it or one of its git worktrees (those the
//! repository itself vouches for, see `monocode_core::git::scoped_worktrees`). It is asked
//! again right before each write, of EVERY process of the foreground group (a `cd` inside the
//! shell counts, and so does `true | (cd / && exec zsh)`, whose leader is gone). On macOS and
//! Linux a lookup that fails is a refusal; only where the system has no lookup at all
//! (Windows) the folder the terminal started in is used. A Mono only types into terminals it
//! opened, and never into one that runs, or ever ran, an agent CLI.
//!
//! Reading is wider: a Mono reads every terminal of its project, the user's and other Monos'
//! included, and an ordinary coding session reads, read-only, the terminals inside its own
//! project or worktree (`session_terminals_call`). Terminals that are not plain shells (agent
//! sessions, logins, cloud shells, `FixTerminal`) and those that run or ever ran an agent CLI
//! are never listed or read. Where the system cannot inspect processes (Windows) an agent CLI
//! cannot be told apart, so only the Mono's own terminals are listed or read there.
//!
//! A Mono's terminals are not saved: a restart brings none back (see `workspace::TerminalPane`).
use super::*;
use crate::{
    pane_layout::{Edge, Preset, preset_layout},
    terminal::{Front, Member, MonoKey, Terminal, group_agent, tail_lines},
    workspace::TerminalPane,
};
use monocode_core::mono::api::is_format_char;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// The most bytes of a free `command` or typed `text`.
const INPUT_MAX: usize = monocode_core::mono::api::TERMINAL_INPUT_MAX;
const LAYOUT_MAX: usize = monocode_core::mono::api::TERMINAL_LAYOUT_MAX;
const LABEL_MAX: usize = 40;
const READ_LINES_DEFAULT: usize = 200;
const READ_LINES_MAX: usize = 1000;
const READ_BYTES_MAX: usize = 32 * 1024;
/// How long `terminals.stop` waits after Ctrl-C before it hangs the job up.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// What a session that is not a Mono gets when it asks for more than a read.
const SESSIONS_READ_ONLY: &str = "Only a Mono can open or drive terminals";

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

impl Registry {
    /// The Mono that opened `terminal`, if one did.
    pub(super) fn mono_of(&self, terminal: &Entity<Terminal>) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.terminal.entity_id() == terminal.entity_id())
            .filter(|e| e.origin == Origin::Mono)
            .and_then(|e| e.mono.clone())
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
    /// The Mono that opened it, for a Mono terminal.
    mono: Option<String>,
    terminal: Entity<Terminal>,
    spot: Spot,
}

/// Who asks for a listing or a read, which decides what it may see.
#[derive(Clone, Copy)]
enum Viewer<'a> {
    /// A Mono: every terminal of the project, to read; only its own to drive.
    Mono(&'a str),
    /// A coding session: every terminal of its project that lies inside `roots` (the
    /// project and the session's worktree).
    Session(&'a [String]),
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

/// Whether `path` is `root` or inside it, compared component by component (bytes on unix, so
/// two names that differ only in non-UTF-8 bytes never collide). A file system root is never
/// a root of a scope: everything is inside `/`.
fn inside(root: &Path, path: &Path) -> bool {
    let fold = |p: &Path| {
        if cfg!(windows) {
            PathBuf::from(p.to_string_lossy().to_lowercase())
        } else {
            p.to_path_buf()
        }
    };
    let root = fold(root);
    root.components().any(|c| matches!(c, Component::Normal(_))) && fold(path).starts_with(&root)
}

/// A folder's identity for comparing two spellings of it: symlinks resolved. A folder that is
/// gone keeps its name under the nearest ancestor that exists.
fn real_path(path: &str) -> PathBuf {
    let mut rest = Vec::new();
    let mut head = Path::new(path);
    loop {
        if let Ok(real) = std::fs::canonicalize(head) {
            break rest
                .iter()
                .rev()
                .fold(real, |real, part: &&std::ffi::OsStr| real.join(part));
        }
        match (head.parent(), head.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                head = parent;
            }
            _ => break Path::new(path).to_path_buf(),
        }
    }
}

/// Whether `cwd` is one of `roots` or inside one, symlinks resolved.
fn in_roots(roots: &[String], cwd: &str) -> bool {
    let real = real_path(cwd);
    roots.iter().any(|root| inside(&real_path(root), &real))
}

/// The git worktrees that count as part of `project`'s scope (see
/// `monocode_core::git::scoped_worktrees`). Only UTF-8 names, since the scope is spelled in
/// text; a worktree whose name is not text is left out.
fn project_worktrees(project: &str) -> Vec<String> {
    monocode_core::git::scoped_worktrees(Path::new(project))
        .into_iter()
        .filter_map(|tree| tree.to_str().map(str::to_owned))
        .collect()
}

/// The folders a Mono's terminals may stand in: the project and its scoped git worktrees.
/// The worktrees are asked of git once, on first use, and kept for the call that built it
/// (a call checks many processes and must not spawn `git` for each).
struct ProjectScope<'a> {
    project: &'a str,
    worktrees: std::cell::OnceCell<Vec<String>>,
}

impl<'a> ProjectScope<'a> {
    fn new(project: &'a str) -> Self {
        Self {
            project,
            worktrees: std::cell::OnceCell::new(),
        }
    }

    /// Whether a folder is the project or inside it or one of its worktrees, symlinks
    /// resolved. A folder reached through `..` is not. A folder that is gone is compared by
    /// its name, since it cannot be resolved.
    fn allows(&self, cwd: &str) -> bool {
        if Path::new(cwd)
            .components()
            .any(|c| c == Component::ParentDir)
        {
            return false;
        }
        let real = real_path(cwd);
        let within = |root: &str| inside(&real_path(root), &real);
        within(self.project)
            || self
                .worktrees
                .get_or_init(|| project_worktrees(self.project))
                .iter()
                .any(|tree| within(tree))
    }
}

#[cfg(test)]
fn folder_allowed(project: &str, cwd: &str) -> bool {
    ProjectScope::new(project).allows(cwd)
}

/// Whether the system can tell what runs in a terminal and where. Where it cannot (Windows),
/// an agent CLI cannot be recognised, so a terminal the user opened is never listed or read,
/// and a write is judged by the folder the terminal started in.
const CAN_INSPECT: bool = cfg!(unix);

const CANNOT_TELL: &str = "Mono cannot tell where this terminal is right now (a process holding it hides its folder or the lookup failed), so it does not type into it. Close it and open a new one with terminals.open";
const AGENT_WRITE: &str = "This terminal runs, or ran, an agent CLI; Mono never types into those";
const AGENT_READ: &str =
    "This terminal runs, or ran, an agent CLI; Mono never reads or types into those";

const OUTSIDE: &str = "The terminal's folder is outside the project and its worktrees, so Mono does not type into it. Stop or close it, or open a new terminal with terminals.open";

/// Whether a terminal with this front is idle enough for `terminals.open` to reuse it: only
/// a shell at its prompt. A front the system could not tell (`Unknown`: the lookup failed, or
/// nothing can be inspected off unix) is never reused, so a running program is not taken for
/// an idle shell; a new terminal opens under the same label (terminals are found by id,
/// never by label).
fn idle_for_reuse(front: &Front) -> bool {
    matches!(front, Front::Shell(_))
}

/// Multi-line input is one bracketed paste when the program asked for that, else each line
/// would run as it arrives: refused. This is about what the terminal does with the text, not
/// about who allowed it, so it holds now that nothing asks the user.
fn multiline_refusal(text: &str, bracketed: bool) -> Option<&'static str> {
    (text.contains('\n') && !bracketed).then_some(
        "This terminal is not in bracketed paste mode, so a line break would run each line as it arrives. Send one line at a time",
    )
}

/// The directory a Mono terminal starts in: the project, or `requested` when it stays inside
/// the project or one of its worktrees (`worktrees` is asked only then). `..` is refused
/// outright and symlinks are resolved before the comparison. A requested folder is returned
/// as it was resolved, so the terminal starts exactly where it was checked; a folder that is
/// not there is an error, never a fallback to the home folder.
fn resolve_cwd(
    project: &str,
    requested: Option<&str>,
    worktrees: impl FnOnce() -> Vec<String>,
) -> Result<String, String> {
    let Some(requested) = requested.map(str::trim) else {
        // The terminal starts strictly in it (`Terminal::new_strict`): a project folder that
        // is gone fails the open there, it does not become a shell in the home folder.
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
    let within = |root: &str| std::fs::canonicalize(root).is_ok_and(|root| inside(&root, &real));
    if within(project) || worktrees().iter().any(|tree| within(tree)) {
        return real
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| "cwd must be a UTF-8 path".into());
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

/// `command` or `text`: text, not empty, at most 4096 bytes, and no control character except
/// line breaks. ESC, Tab, Enter (`\r`), Ctrl-C and the rest are keys, never text, so a Mono
/// cannot type an escape sequence or ask the shell to complete.
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

/// How many lines a read asks for.
fn parse_lines(input: &Value) -> Result<usize, String> {
    match input.get("lines") {
        None => Ok(READ_LINES_DEFAULT),
        Some(v) => match v.as_u64() {
            Some(0) | None => Err("lines must be a positive integer".into()),
            Some(n) => Ok((n as usize).min(READ_LINES_MAX)),
        },
    }
}

/// What a read of `t` answers: the output with secrets masked, cut to the last lines.
fn read_answer(t: &Terminal, lines: usize) -> Value {
    // Masked over the whole scrollback first, then cut: a secret that starts above the
    // window (a private key's first line) must not survive as an unmarked tail.
    let all = monocode_core::mono::memory::redact_output(&t.scrollback_text());
    let (text, older) = tail_lines(&all, lines);
    let (text, capped) = cap_tail(text, READ_BYTES_MAX);
    json!({
        "text": text,
        "truncated": older || capped,
        "running": t.foreground.is_some() && !t.exited,
        "exited": t.exited,
        "exitCode": t.exit_code,
    })
}

/// Whether `t` is never listed or read: a launched program (an agent session, a login, a
/// cloud shell, `FixTerminal`) is not a plain shell, and an agent CLI typed into a shell holds
/// the user's conversation, now or earlier (the scrollback keeps it). `group` is the probe the
/// caller just made of the foreground group.
fn hidden(t: &Terminal, group: Option<&[Member]>) -> bool {
    !t.is_plain_shell() || t.note_agent(group)
}

/// Whether a terminal of `origin` opened by `owner` may be listed or read by `viewer`: where
/// the system cannot inspect processes an agent CLI in a terminal the user opened cannot be
/// told, and another Mono's terminal cannot be vouched for either, so only the calling Mono's
/// own are shown. A coding session has no terminals of its own: it gets none there.
fn readable_origin(origin: Origin, owner: Option<&str>, viewer: Viewer, can_inspect: bool) -> bool {
    can_inspect
        || (origin == Origin::Mono && matches!(viewer, Viewer::Mono(me) if owner == Some(me)))
}

/// Whether a terminal lies inside `roots`, for a coding session's read: every process of the
/// foreground group must, and a group that cannot be listed is out. Where nothing can be
/// inspected, the folder the terminal started in.
fn within_roots(
    group: Option<&[Member]>,
    start: &str,
    roots: &[String],
    can_inspect: bool,
) -> bool {
    if !can_inspect {
        return in_roots(roots, start);
    }
    group.is_some_and(|group| {
        !group.is_empty()
            && group
                .iter()
                .all(|m| m.cwd.as_deref().is_some_and(|cwd| in_roots(roots, cwd)))
    })
}

/// The scope check of a write: nothing in the foreground group may be an agent CLI, and every
/// process of it must stand in a folder `allowed` accepts. A group that cannot be listed, or a
/// member whose folder is unknown, is a refusal. Only where nothing can be inspected
/// (`can_inspect` false) is the folder the terminal started in used.
fn write_gate(
    group: Option<&[Member]>,
    agent: bool,
    start: &str,
    can_inspect: bool,
    allowed: impl Fn(&str) -> bool,
) -> Result<(), String> {
    if agent || group.is_some_and(|g| group_agent(g).is_some()) {
        return Err(AGENT_WRITE.into());
    }
    if !can_inspect {
        return if allowed(start) {
            Ok(())
        } else {
            Err(OUTSIDE.into())
        };
    }
    let Some(group) = group.filter(|g| !g.is_empty()) else {
        return Err(CANNOT_TELL.into());
    };
    for member in group {
        match member.cwd.as_deref() {
            None => return Err(CANNOT_TELL.into()),
            Some(cwd) if !allowed(cwd) => return Err(OUTSIDE.into()),
            Some(_) => {}
        }
    }
    Ok(())
}

impl Shell {
    pub(crate) fn mono_terminals_call(
        &mut self,
        mono: &str,
        project: &str,
        action: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let project = self.workspace_project_key(project);
        let project = project.as_str();
        if project.starts_with("remote://") {
            return Err("Terminals are not available on remote projects".into());
        }
        self.prune_mono_terminals();
        match action {
            "terminals.list" => Ok(self.list_terminals(project, Viewer::Mono(mono), cx)),
            "terminals.open" => self.mono_open_terminal(mono, project, input, cx),
            "terminals.send" => self.mono_send_terminal(mono, project, input, cx),
            "terminals.read" => self.mono_read_terminal(mono, project, input, cx),
            "terminals.stop" => self.mono_stop_terminal(mono, project, input, cx),
            "terminals.close" => self.mono_close_terminal(mono, project, input, cx),
            "terminals.layout" => self.mono_layout_terminals(mono, project, input, cx),
            _ => Err("Unknown terminals action".into()),
        }
    }

    /// Where the keyboard goes after a `terminals.*` call selected another title tab (what
    /// had it may be hidden now). Call it with the window once the call has run.
    pub(crate) fn settle_mono_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.mono_switched) {
            let held = window.focused(cx);
            let target = self.focus_after_switch(held.as_ref(), window, cx);
            window.focus(&target, cx);
        }
    }

    /// Where the keyboard goes after a call selected another title tab: `saved` (what had it
    /// before) when it is a surface the switch does not hide (the Mono's composer or its
    /// details panel), else the shell root. The same rule serves a new pane and a reused one.
    fn focus_after_switch(
        &self,
        saved: Option<&FocusHandle>,
        window: &Window,
        cx: &App,
    ) -> FocusHandle {
        let still_shown = |saved: &FocusHandle| {
            let mut surfaces = Vec::new();
            if let Some(monos) = self.monos.as_ref().map(|m| m.read(cx)) {
                surfaces.push(monos.details.focus_handle(cx));
                if let Some(chat) = monos.chat() {
                    surfaces.push(chat.read(cx).input.focus_handle(cx));
                }
            }
            if self.workspace.right_panel_open
                && let Some(panel) = &self.right_panel
            {
                surfaces.push(panel.focus_handle(cx));
            }
            surfaces
                .iter()
                .any(|surface| surface.contains(saved, window))
        };
        saved
            .filter(|saved| still_shown(saved))
            .cloned()
            .unwrap_or_else(|| self.focus.clone())
    }

    /// What an ordinary coding session may do with terminals, which is read them: only
    /// `terminals.list` and `terminals.read`, only for a local session, and only for the
    /// terminals inside its project or its worktree. Returns the answer.
    pub(super) fn session_terminals_call(
        &mut self,
        session: i64,
        action: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let (project, roots) = self.session_terminal_scope(session, action, input, cx)?;
        self.prune_mono_terminals();
        let viewer = Viewer::Session(&roots);
        if action == "terminals.list" {
            return Ok(self.list_terminals(&project, viewer, cx));
        }
        let (id, lines) = (id_of(input)?, parse_lines(input)?);
        let found = self.find_terminal(viewer, &project, &id, true, cx)?;
        Ok(read_answer(found.terminal.read(cx), lines))
    }

    /// The project (as the workspace knows it) and the folders a session may see terminals
    /// in: its project root and its worktree. Refuses everything that is not a read, a remote
    /// or cloud session, and another project.
    pub(super) fn session_terminal_scope(
        &self,
        session: i64,
        action: &str,
        input: &Value,
        cx: &App,
    ) -> Result<(String, Vec<String>), String> {
        let view = self
            .sessions
            .get(&session)
            .map(|v| v.read(cx))
            .ok_or("Unknown session.")?;
        if view.summary.cwd.starts_with("remote://")
            || self.is_cloud(session)
            || self.remote_marks.contains_key(&session)
        {
            return Err("Terminals are not available on remote or cloud sessions".into());
        }
        if !matches!(action, "terminals.list" | "terminals.read") {
            return Err(SESSIONS_READ_ONLY.into());
        }
        monocode_core::mono::api::validate(action, input)?;
        let root = view.summary.cwd.clone();
        if input
            .get("project")
            .and_then(Value::as_str)
            .is_some_and(|p| {
                monocode_core::files::path_key(p) != monocode_core::files::path_key(&root)
            })
        {
            return Err("A session can only read terminals of its own project.".into());
        }
        let mut roots = vec![root.clone()];
        roots.extend(view.settings.worktree_cwd.clone());
        Ok((self.workspace_project_key(&root), roots))
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
            terminal: terminal.downgrade(),
        });
        id
    }

    /// A terminal `viewer` may read: in the project, plain, never one that runs or ran an
    /// agent CLI, and for a session inside its roots (a Mono reads the whole project).
    /// Anything not in the registry is unknown.
    fn find_terminal(
        &self,
        viewer: Viewer,
        project: &str,
        id: &str,
        reading: bool,
        cx: &App,
    ) -> Result<Found, String> {
        let unknown = || format!("Unknown terminal {id}. Run terminals.list");
        let entry = self
            .mono_registry
            .entries
            .iter()
            .find(|e| e.id == id)
            .filter(|e| {
                monocode_core::files::path_key(&e.project)
                    == monocode_core::files::path_key(project)
                    && (!reading
                        || readable_origin(e.origin, e.mono.as_deref(), viewer, CAN_INSPECT))
            })
            .ok_or_else(unknown)?;
        let terminal = entry.terminal.upgrade().ok_or_else(unknown)?;
        let spot = self.terminal_spot(project, &terminal).ok_or_else(unknown)?;
        let t = terminal.read(cx);
        if !t.is_plain_shell() {
            return Err(unknown());
        }
        // Only a read probes here; typing asks the PTY itself (`mono_send_terminal`), and
        // stopping or closing a terminal that once ran an agent stays possible.
        if reading {
            let group = t.live_group();
            if t.note_agent(group.as_deref()) {
                return Err(AGENT_READ.into());
            }
            if let Viewer::Session(roots) = viewer
                && !within_roots(group.as_deref(), &t.cwd, roots, CAN_INSPECT)
            {
                return Err(unknown());
            }
        }
        Ok(Found {
            origin: entry.origin,
            mono: entry.mono.clone(),
            terminal,
            spot,
        })
    }

    /// A terminal this Mono may address (see `find_terminal`).
    fn find_mono_terminal(
        &self,
        mono: &str,
        project: &str,
        id: &str,
        cx: &App,
    ) -> Result<Found, String> {
        self.find_terminal(Viewer::Mono(mono), project, id, true, cx)
    }

    /// Like `find_mono_terminal`, for what only the terminal's opener may do.
    fn find_own_terminal(
        &self,
        mono: &str,
        project: &str,
        id: &str,
        cx: &App,
    ) -> Result<Found, String> {
        let found = self.find_terminal(Viewer::Mono(mono), project, id, false, cx)?;
        if found.origin != Origin::Mono {
            return Err(
                "A terminal the user opened cannot be typed into, stopped or closed".into(),
            );
        }
        if found.mono.as_deref() != Some(mono) {
            return Err(
                "Another Mono opened this terminal: only it can type into, stop or close it".into(),
            );
        }
        Ok(found)
    }

    /// The terminal `beside` names: one of this Mono's own. A user's terminal pane may sit in
    /// the user's chat tab, and a Mono never splits that.
    fn find_beside(&self, mono: &str, project: &str, id: &str, cx: &App) -> Result<Found, String> {
        self.find_own_terminal(mono, project, id, cx).map_err(|_| {
            "beside must be a terminal this Mono opened (see terminals.list); the user's terminals are never split".to_owned()
        })
    }

    /// Every terminal of `project` in the dock and in panes, with where it is.
    fn project_terminals(&self, project: &str) -> Vec<(Entity<Terminal>, Spot)> {
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
        spots
    }

    /// `terminals.list`: what `viewer` may see of the project's terminals.
    fn list_terminals(&mut self, project: &str, viewer: Viewer, cx: &App) -> Value {
        let mut rows = Vec::new();
        for (terminal, spot) in self.project_terminals(project) {
            let t = terminal.read(cx);
            if !t.is_plain_shell() {
                continue;
            }
            // One probe of the process group serves the agent check and the scope check.
            let group = t.live_group();
            if hidden(t, group.as_deref()) {
                continue;
            }
            if let Viewer::Session(roots) = viewer
                && !within_roots(group.as_deref(), &t.cwd, roots, CAN_INSPECT)
            {
                continue;
            }
            let known = self
                .mono_registry
                .entries
                .iter()
                .find(|e| e.terminal.entity_id() == terminal.entity_id())
                .map(|e| (e.origin, e.mono.clone()));
            let origin = known.as_ref().map_or(Origin::User, |(origin, _)| *origin);
            let owner = known.as_ref().and_then(|(_, owner)| owner.as_deref());
            if !readable_origin(origin, owner, viewer, CAN_INSPECT) {
                continue;
            }
            let mine = matches!(
                (&known, viewer),
                (Some((Origin::Mono, Some(owner))), Viewer::Mono(me)) if owner == me
            );
            let id = self.registry_id(&terminal, origin, None, project);
            let t = terminal.read(cx);
            let place = self.place_name(spot);
            let mut row = json!({
                "id": id,
                "label": t.display_title(),
                "cwd": t.cwd,
                "place": place,
                "running": t.foreground.is_some() && !t.exited,
                "exited": t.exited,
                "exitCode": t.exit_code,
                "owner": origin.name(),
            });
            // A Mono reads the other Monos' terminals too, and tells which are its own.
            if matches!(viewer, Viewer::Mono(_)) {
                row["mine"] = json!(mine);
            }
            rows.push(row);
        }
        Value::Array(rows)
    }

    /// The ONE place a command a Mono terminal runs reaches its shell: a saved Project Action
    /// (the user wrote it) or a free command. Nothing else types a command into a Mono
    /// terminal.
    fn run_mono_input(terminal: &Entity<Terminal>, run: &Run, cx: &mut Context<Self>) {
        terminal.update(cx, |t, cx| match run {
            Run::Action(command) => t.run(command, cx),
            Run::Command(command) => t.send_text(command, true, cx),
        });
    }

    fn mono_open_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
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
        let place = match input.get("place") {
            None => None,
            Some(Value::String(place)) if matches!(place.as_str(), "dock" | "tab" | "split") => {
                Some(place.as_str())
            }
            Some(_) => return Err("place must be \"dock\", \"tab\" or \"split\"".into()),
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
            Some(id) => Some(self.find_beside(mono, project, id, cx)?),
        };
        if beside.is_some() && matches!(place, Some("dock" | "tab")) {
            return Err("beside goes with place \"split\" (or no place)".into());
        }
        let run = match (&action, &command) {
            (Some(action), _) => Some(Run::Action(action.command.clone())),
            (None, Some(command)) => Some(Run::Command(command.clone())),
            (None, None) => None,
        };
        // An idle terminal with this label is reused, like an action's own terminal, when it
        // is the Mono's, stands at its shell prompt right now (not as of the last poll) and is
        // in the folder asked for (the project's own when none was). Where it stands is asked
        // of the system when it can tell, else it is where it started.
        let wanted = real_path(&cwd);
        let scope = ProjectScope::new(project);
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
            let group = t.live_group();
            let at_prompt = idle_for_reuse(&t.front_in(group.as_deref()));
            // Where every process of the group stands, asked of the system when it can tell
            // (else where the terminal started); a lookup that fails is no reuse.
            let folders: Option<Vec<String>> = if CAN_INSPECT {
                group
                    .as_ref()
                    .filter(|g| !g.is_empty())
                    .and_then(|g| g.iter().map(|m| m.cwd.clone()).collect::<Option<Vec<_>>>())
            } else {
                Some(vec![t.cwd.clone()])
            };
            let same = at_prompt
                && !t.note_agent(group.as_deref())
                && folders.is_some_and(|folders| {
                    folders
                        .iter()
                        .all(|folder| real_path(folder) == wanted && scope.allows(folder))
                });
            same.then(|| (entry.id.clone(), terminal.clone(), t.bracketed_paste()))
        });
        let reusable =
            reusable.filter(|(_, terminal, _)| self.terminal_spot(project, terminal).is_some());
        if let Some(Run::Command(command)) = &run {
            // A multi-line command is a paste, which only a shell that asked for one takes.
            // A terminal that is not there yet has asked for nothing.
            let bracketed = reusable
                .as_ref()
                .is_some_and(|(_, _, bracketed)| *bracketed);
            if let Some(refusal) = multiline_refusal(command, bracketed) {
                return Err(refusal.into());
            }
        }
        if let Some((id, terminal, _)) = reusable
            && let Some(spot) = self.terminal_spot(project, &terminal)
        {
            if let Some(run) = &run {
                Self::run_mono_input(&terminal, run, cx);
            }
            let revealed = !reveal || self.reveal_mono_terminal(project, spot, switch, from, cx);
            let mut done = json!({"id": id, "reused": true, "place": self.place_name(spot)});
            if !revealed {
                done["revealed"] = json!(false);
            }
            return Ok(done);
        }

        let in_panel = self.terminal_in_panel(cx);
        // Beside a pane means a pane, whatever the user's default says.
        let place = place.unwrap_or_else(|| {
            if matches!(&beside, Some(b) if matches!(b.spot, Spot::Pane(_)))
                || (self.workspace.preferences.terminal_placement() == "title" && !in_panel)
            {
                "split"
            } else {
                "dock"
            }
        });
        let opened = match place {
            "split" => {
                let target = self.mono_split_target(mono, project, beside.as_ref())?;
                self.open_mono_terminal_pane(
                    mono, project, &label, &cwd, target, edge, reveal, switch, false, cx,
                )
            }
            "tab" => self.open_mono_terminal_pane(
                mono, project, &label, &cwd, None, edge, reveal, switch, true, cx,
            ),
            _ => None,
        };
        let (terminal, spot) = match opened {
            Some(opened) => opened,
            None => self.open_mono_dock_terminal(project, &label, &cwd, reveal, in_panel, cx),
        };
        // A folder that vanished since it was checked is a failed open, not a shell elsewhere.
        if let Some(problem) = terminal.read(cx).spawn_problem().map(str::to_owned) {
            match spot {
                Spot::Dock(ix) => self.close_mono_dock_terminal(project, ix, cx),
                Spot::Pane(pane) => self.close_mono_pane(pane, cx),
            }
            return Err(format!("The terminal could not start in {cwd}: {problem}"));
        }
        let id = self.registry_id(&terminal, Origin::Mono, Some(mono), project);
        if let Some(run) = &run {
            Self::run_mono_input(&terminal, run, cx);
        }
        let revealed = !reveal || self.reveal_mono_terminal(project, spot, switch, from, cx);
        cx.notify();
        let mut done = json!({"id": id, "reused": false, "place": self.place_name(spot)});
        if !revealed {
            done["revealed"] = json!(false);
        }
        Ok(done)
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

    /// A new terminal pane. `own_tab` keeps it out of the Mono's Logs tab: a title tab of its
    /// own that later splits do not join. `None` when the project has no session to lend its
    /// scope: the caller then falls back to the dock.
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
        own_tab: bool,
        cx: &mut Context<Self>,
    ) -> Option<(Entity<Terminal>, Spot)> {
        let owner = self.mono_pane_owner(project, None)?;
        let terminal = self.spawn_mono_terminal(cwd.to_owned(), cx);
        terminal.update(cx, |t, _| {
            t.label = Some(label.to_owned());
            t.by_mono = true;
        });
        let pane = self.workspace.next_pane_id();
        let mut state = TerminalPane::new(owner, cwd.to_owned());
        state.mono = Some(mono.to_owned());
        // Joining the Mono's Logs tab keeps it a Logs tab, and a tab of its own is one unless
        // it was asked to be a plain tab.
        state.logs = match target.filter(|t| self.workspace.tab_of(*t).is_some()) {
            None => (!own_tab).then(|| logs_title(project)),
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
        self.cancel_dock_gestures(project, cx);
        let viewport = self.viewport();
        let dock = self.workspace.docks.entry(project.to_owned()).or_default();
        let before = (!dock.terminals.is_empty()).then_some((dock.active, dock.open));
        dock.size = dock.side.clamp(dock.size, viewport);
        let ix = dock.add(cwd.to_owned());
        dock.set_mono(ix, true);
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
        let terminal = self.spawn_mono_terminal(cwd.to_owned(), cx);
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
                    self.mono_switched = true;
                }
                true
            }
        }
    }

    /// `terminals.layout`: the given terminals of this Mono in one "Logs <project>" tab, from
    /// wherever they are (the dock, title tabs of their own or splits).
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
            terminals.push(self.find_own_terminal(mono, project, id, cx)?.terminal);
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
            if let Some(state) = self.workspace.terminal_panes.get_mut(&pane) {
                state.mono = Some(mono.to_owned());
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
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
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
        let found = self.find_own_terminal(mono, project, &id, cx)?;
        // Everything below is asked of the terminal now: the poll behind `foreground` can be
        // a second old.
        let t = found.terminal.read(cx);
        if t.exited {
            return Err("The terminal has exited".into());
        }
        // One probe of the foreground group now, for the agent check and the scope rule at
        // write time: wherever every process of it is now, not where the terminal was opened.
        let group = t.live_group();
        let agent = t.note_agent(group.as_deref());
        let scope = ProjectScope::new(project);
        write_gate(group.as_deref(), agent, &t.cwd, CAN_INSPECT, |folder| {
            scope.allows(folder)
        })?;
        if let Some(text) = &text
            && let Some(refusal) = multiline_refusal(text, t.bracketed_paste())
        {
            return Err(refusal.into());
        }
        if let Some(text) = &text {
            found
                .terminal
                .update(cx, |t, cx| t.send_text(text, false, cx));
        }
        if let Some((_, key)) = key {
            found.terminal.update(cx, |t, cx| t.send_key(key, cx));
        }
        Ok(match (&text, key) {
            (None, Some((name, _))) => json!({"sent": name}),
            (Some(text), key) => {
                json!({"sent": "text", "bytes": text.len(), "key": key.map(|(name, _)| name)})
            }
            (None, None) => unreachable!("checked above"),
        })
    }

    fn mono_read_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let id = id_of(input)?;
        let lines = parse_lines(input)?;
        let found = self.find_mono_terminal(mono, project, &id, cx)?;
        Ok(read_answer(found.terminal.read(cx), lines))
    }

    fn mono_stop_terminal(
        &mut self,
        mono: &str,
        project: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let found = self.find_own_terminal(mono, project, &id_of(input)?, cx)?;
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
        let found = self.find_own_terminal(mono, project, &id_of(input)?, cx)?;
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
    /// none is left, keeping its shell, label and Mono. Call before the session's summary is
    /// dropped (the pane's project is the owner's).
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

    /// A Mono terminal pane becomes a tab of its project's dock, keeping its live shell. The
    /// dock stays as the user had it.
    fn move_mono_pane_to_dock(&mut self, pane: i64, project: &str, cx: &mut Context<Self>) {
        let Some(state) = self.workspace.terminal_panes.get(&pane).cloned() else {
            return;
        };
        let Some(view) = self.terminal_pane_view(pane, cx) else {
            return;
        };
        if state.mono.is_none() {
            return;
        }
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
        let dock = self.workspace.docks.entry(project.to_owned()).or_default();
        let before = (!dock.terminals.is_empty()).then_some((dock.active, dock.open));
        let ix = dock.add(state.cwd.clone());
        dock.set_mono(ix, true);
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
}

/// What a call types into a Mono terminal.
enum Run {
    /// The command of a saved Project Action (the user wrote it).
    Action(String),
    /// A free command.
    Command(String),
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
        window
            .update(cx, |shell, w, cx| {
                let result = shell.mono_terminals_call(mono, PROJECT, action, &input, cx);
                shell.settle_mono_focus(w, cx);
                result
            })
            .unwrap()
    }

    fn terminal(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        id: &str,
    ) -> Entity<Terminal> {
        window
            .update(cx, |shell, _, _| {
                shell
                    .mono_registry
                    .entries
                    .iter()
                    .find(|e| e.id == id)
                    .and_then(|e| e.terminal.upgrade())
                    .unwrap()
            })
            .unwrap()
    }

    /// Where the terminal with registry id `id` is.
    fn spot_of(shell: &Shell, id: &str) -> Spot {
        let entry = shell
            .mono_registry
            .entries
            .iter()
            .find(|e| e.id == id)
            .unwrap();
        shell
            .terminal_spot(&entry.project, &entry.terminal.upgrade().unwrap())
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
        // Another Mono's terminal is listed too (read-only), marked as not mine.
        assert_eq!(owners.len(), 3, "{list:?}");
        assert_eq!(owners[0], (mine.as_str(), "mono"));
        assert_eq!(owners[1].1, "mono");
        assert_eq!(owners[2].1, "user");
        assert_eq!(list[0]["mine"], true);
        assert_eq!(list[1]["mine"], false);
        assert_eq!(list[1]["label"], "theirs");
        assert_eq!(list[2]["mine"], false);
        let user_id = owners[2].0.to_owned();
        assert_eq!(terminal(window, cx, &user_id), user);
        assert_eq!(list[0]["place"], "dock");
        assert_eq!(list[0]["running"], false);
    }

    #[gpui::test]
    fn a_mono_reads_another_monos_terminal_but_cannot_drive_it(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let theirs = dock_as(window, cx, "m2", "theirs");
        let t = terminal(window, cx, &theirs);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| t.fake_output("build ok\r\n", cx))
            })
            .unwrap();
        let read = call(window, cx, "m1", "terminals.read", json!({"id": theirs})).unwrap();
        assert!(
            read["text"].as_str().unwrap().contains("build ok"),
            "{read}"
        );
        for (action, input) in [
            ("terminals.send", json!({"id": theirs, "key": "ctrl-c"})),
            ("terminals.stop", json!({"id": theirs})),
            ("terminals.close", json!({"id": theirs})),
        ] {
            let refused = call(window, cx, "m1", action, input).unwrap_err();
            assert!(
                refused.contains("Another Mono opened"),
                "{action}: {refused}"
            );
        }
        assert!(sent(window, cx, &theirs).is_empty());
        let beside = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "x", "place": "split", "beside": theirs}),
        )
        .unwrap_err();
        assert!(
            beside.contains("beside must be a terminal this Mono opened"),
            "{beside}"
        );
        // The owner still can.
        call(
            window,
            cx,
            "m2",
            "terminals.send",
            json!({"id": theirs, "key": "ctrl-c"}),
        )
        .unwrap();
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
                let Spot::Pane(p) = spot_of(shell, &pane) else {
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
                let Spot::Pane(p) = spot_of(shell, &be) else {
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
            assert_eq!(real_path(&real), real_path(&link));
            let _ = std::fs::remove_dir_all(&dir);
        }
        // A folder that does not exist is compared as spelled.
        assert_eq!(real_path("/no/such/dir/"), Path::new("/no/such/dir"));
    }

    fn git(cwd: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(cwd)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .status()
            .expect("git is installed");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_planted_worktree_or_a_parent_repository_never_widens_the_scope() {
        let root = std::fs::canonicalize(scratch("wide")).unwrap();
        let repo = root.join("repo");
        let package = repo.join("packages/api");
        std::fs::create_dir_all(&package).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["commit", "--allow-empty", "-m", "Initial"]);
        let sibling = repo.join("packages/web");
        std::fs::create_dir_all(&sibling).unwrap();
        // The project is a package of a repository: the repository is not its worktree.
        let (package_text, sibling_text) = (
            package.to_string_lossy().into_owned(),
            sibling.to_string_lossy().into_owned(),
        );
        assert!(folder_allowed(&package_text, &package_text));
        assert!(!folder_allowed(&package_text, &sibling_text));
        assert!(!folder_allowed(&package_text, &repo.to_string_lossy()));
        // A repository whose `.git` registers `/` as a worktree: `/` is still outside.
        let admin = repo.join(".git/worktrees/root");
        std::fs::create_dir_all(&admin).unwrap();
        // `//.git` is the spelling that makes git list `/` (`/.git` does not).
        std::fs::write(admin.join("gitdir"), "//.git\n").unwrap();
        std::fs::write(admin.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let repo_text = repo.to_string_lossy().into_owned();
        assert!(folder_allowed(&repo_text, &format!("{repo_text}/packages")));
        for outside in ["/", "/etc", &root.to_string_lossy()] {
            assert!(!folder_allowed(&repo_text, outside), "{outside}");
        }
        let listed = monocode_core::git::worktrees(&repo).unwrap();
        assert!(
            listed.iter().any(|tree| tree.path == "/" && !tree.missing),
            "git must list / for the planted entry"
        );
        assert!(project_worktrees(&repo_text).iter().all(|t| t != "/"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_system_root_is_never_the_root_of_a_scope() {
        assert!(inside(Path::new("/work"), Path::new("/work")));
        assert!(inside(Path::new("/work"), Path::new("/work/app")));
        assert!(!inside(Path::new("/work"), Path::new("/workshop")));
        assert!(!inside(Path::new("/"), Path::new("/")));
        assert!(!inside(Path::new("/"), Path::new("/etc")));
        assert!(!in_roots(&["/".to_owned()], "/etc"));
        assert!(!folder_allowed("/", "/etc"));
        #[cfg(windows)]
        assert!(!inside(Path::new("C:\\"), Path::new("C:\\Users")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn names_that_differ_only_in_non_utf8_bytes_do_not_collide() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let root = Path::new(OsStr::from_bytes(b"/work/a\xff"));
        let same = Path::new(OsStr::from_bytes(b"/work/a\xff/sub"));
        let other = Path::new(OsStr::from_bytes(b"/work/a\xfe/sub"));
        // Both would read "/work/a\u{fffd}" through a lossy conversion.
        assert_eq!(
            root.to_string_lossy(),
            Path::new("/work/a\u{fffd}").to_string_lossy()
        );
        assert!(inside(root, same));
        assert!(!inside(root, other));
        // A real folder pair that differs only in such bytes stays two folders.
        let dir = std::env::temp_dir().join(format!("mono-bytes-{}", std::process::id()));
        let (first, second) = (
            dir.join(OsStr::from_bytes(b"d\xff")),
            dir.join(OsStr::from_bytes(b"d\xfe")),
        );
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        assert!(!inside(&first, &second));
        assert!(inside(&first, &first.join("x")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn member(pid: i32, args: &str, cwd: Option<&str>) -> Member {
        Member {
            pid,
            args: args.into(),
            cwd: cwd.map(str::to_owned),
        }
    }

    #[test]
    fn a_write_needs_every_process_in_front_to_be_inside_the_scope() {
        let allowed = |folder: &str| folder.starts_with("/work");
        let gate = |group: Option<Vec<Member>>, agent, can_inspect| {
            write_gate(group.as_deref(), agent, "/work", can_inspect, allowed)
        };
        let ok = vec![member(1, "zsh", Some("/work/app"))];
        assert_eq!(gate(Some(ok.clone()), false, true), Ok(()));
        // A pipeline: the leader is inside, a later process is not.
        let split = vec![
            member(1, "sleep 1e9", Some("/work")),
            member(2, "zsh -i", Some("/")),
        ];
        assert_eq!(gate(Some(split), false, true), Err(OUTSIDE.into()));
        // The leader's folder alone does not decide.
        let one_unknown = vec![
            member(1, "sleep 1e9", Some("/work")),
            member(2, "zsh -i", None),
        ];
        assert_eq!(
            gate(Some(one_unknown), false, true),
            Err(CANNOT_TELL.into())
        );
        // A lookup that failed, or an empty group, is a refusal, never the start folder.
        assert_eq!(gate(None, false, true), Err(CANNOT_TELL.into()));
        assert_eq!(gate(Some(Vec::new()), false, true), Err(CANNOT_TELL.into()));
        // An agent anywhere in the group, or one seen before, is never typed into.
        let piped = vec![
            member(1, "cat prompt.txt", Some("/work")),
            member(2, "claude", Some("/work")),
        ];
        assert_eq!(gate(Some(piped), false, true), Err(AGENT_WRITE.into()));
        assert_eq!(gate(Some(ok), true, true), Err(AGENT_WRITE.into()));
        // Where nothing can be inspected the folder it started in decides (Windows).
        assert_eq!(gate(None, false, false), Ok(()));
        assert_eq!(
            write_gate(None, false, "/tmp", false, allowed),
            Err(OUTSIDE.into())
        );
    }

    #[test]
    fn a_session_sees_a_terminal_only_when_every_process_is_inside_its_roots() {
        let roots = vec!["/work/app".to_owned()];
        let inside_all = vec![member(1, "zsh", Some("/work/app/src"))];
        assert!(within_roots(Some(&inside_all), "/", &roots, true));
        let mixed = vec![
            member(1, "sleep 9", Some("/work/app")),
            member(2, "zsh", Some("/tmp")),
        ];
        assert!(!within_roots(Some(&mixed), "/work/app", &roots, true));
        let unknown = vec![member(1, "zsh", None)];
        assert!(!within_roots(Some(&unknown), "/work/app", &roots, true));
        assert!(!within_roots(None, "/work/app", &roots, true));
        // Nothing to inspect: where it started.
        assert!(within_roots(None, "/work/app", &roots, false));
        assert!(!within_roots(None, "/tmp", &roots, false));
    }

    #[test]
    fn where_nothing_can_be_inspected_only_the_calling_monos_own_terminals_are_readable() {
        let (me, other) = (Viewer::Mono("m1"), Viewer::Mono("m2"));
        // Off unix: the caller's own only; the user's, another Mono's and a session's: none.
        assert!(readable_origin(Origin::Mono, Some("m1"), me, false));
        assert!(!readable_origin(Origin::Mono, Some("m1"), other, false));
        assert!(!readable_origin(Origin::Mono, None, me, false));
        assert!(!readable_origin(Origin::User, None, me, false));
        let roots = ["/work".to_owned()];
        assert!(!readable_origin(
            Origin::Mono,
            Some("m1"),
            Viewer::Session(&roots),
            false
        ));
        // Where processes can be inspected the group check decides, not the origin.
        assert!(readable_origin(Origin::User, None, me, true));
        assert!(readable_origin(Origin::Mono, Some("m1"), other, true));
        assert!(readable_origin(
            Origin::Mono,
            Some("m1"),
            Viewer::Session(&roots),
            true
        ));
    }

    #[test]
    fn only_a_shell_at_its_prompt_is_reused() {
        assert!(idle_for_reuse(&Front::Shell("zsh".into())));
        assert!(!idle_for_reuse(&Front::Program("vite".into())));
        // A lookup that failed is no reuse, whatever the last poll said.
        assert!(!idle_for_reuse(&Front::Unknown));
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
        // The user's and the other Mono's are listed, and nothing new opened.
        assert_eq!(rows(window, cx).len(), 2, "nothing opened");
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

    #[test]
    fn a_folder_is_allowed_when_it_is_the_project_or_below_it() {
        let base = scratch("live-cwd");
        let project = base.join("app");
        let inside = project.join("src");
        let outside = base.join("other");
        for dir in [&inside, &outside] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let project = project.to_string_lossy().into_owned();
        assert!(folder_allowed(&project, &project));
        assert!(folder_allowed(&project, &inside.to_string_lossy()));
        assert!(!folder_allowed(&project, &outside.to_string_lossy()));
        assert!(!folder_allowed(&project, &format!("{project}/../other")));
        // A folder that is gone cannot be resolved: it is compared by its name.
        assert!(folder_allowed(&project, &format!("{project}/gone")));
        assert!(!folder_allowed(
            &project,
            &format!("{}/gone", outside.display())
        ));
        assert!(!folder_allowed(&project, "/"));
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
            .update(cx, |shell, _, _| match spot_of(shell, id) {
                Spot::Pane(pane) => pane,
                Spot::Dock(_) => panic!("{id} is in the dock"),
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
                shell.mono_manager().update(cx, |monos, _| {
                    monos.roster.monos.push(monocode_core::mono::Mono {
                        id: "m1".into(),
                        name: "Mono m1".into(),
                        projects: vec![PROJECT.into()],
                        ..Default::default()
                    })
                });
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
            .update(cx, |shell, _, cx| {
                assert!(!shell.session_summaries.contains_key(&chat));
                assert!(shell.workspace.terminal_panes.is_empty());
                assert!(shell.workspace.tab_of(pane).is_none());
                assert!(shell.workspace.docks[PROJECT].is_mono(0));
                let kept = shell.terminals[PROJECT][0].read(cx);
                assert_eq!((kept.label.as_deref(), kept.by_mono), (Some("BE"), true));
                assert_eq!(
                    shell
                        .mono_registry
                        .mono_of(&shell.terminals[PROJECT][0])
                        .as_deref(),
                    Some("m1")
                );
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
        let quiet = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "split"}),
        )
        .unwrap();
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
                assert!(shell.mono_switched);
            })
            .unwrap();
        window
            .update(cx, |shell, w, cx| shell.settle_mono_focus(w, cx))
            .unwrap();
        window
            .update(cx, |shell, w, cx| {
                let terminal = shell.terminal_pane_views[&pane].focus_handle(cx);
                assert!(w.focused(cx) != Some(terminal));
                assert_eq!(w.focused(cx), Some(shell.focus.clone()));
                assert!(!shell.mono_switched);
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
            // A link that stays inside resolves to the folder it names: the terminal starts
            // in what was checked, not in whatever the link points to later.
            std::os::unix::fs::symlink(project.join("src"), project.join("alias")).unwrap();
            let resolved = resolve_cwd(&project_text, Some("alias"), Vec::new).unwrap();
            assert_eq!(
                Path::new(&resolved),
                std::fs::canonicalize(project.join("src")).unwrap()
            );
        }
        // A project that is `/` makes everything inside it, which is no scope at all.
        assert!(resolve_cwd("/", Some("tmp"), Vec::new).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[gpui::test]
    fn commands_text_and_every_key_are_written_at_once_with_no_dialog(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        // A free command: typed and run in the new terminal right away.
        let be = open(
            window,
            cx,
            json!({"label": "BE", "command": "npm run dev", "place": "dock"}),
        );
        assert_eq!(sent(window, cx, &be), b"npm run dev\r");
        let id = dock(window, cx, "SH");
        // The shell itself holds the terminal: text and every key go through, in order.
        let mut want = Vec::new();
        let done = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls -la", "key": "enter"}),
        )
        .unwrap();
        assert_eq!(done["key"], "enter");
        want.extend_from_slice(b"ls -la\r");
        assert_eq!(sent(window, cx, &id), want);
        for (key, bytes) in [
            ("ctrl-c", &b"\x03"[..]),
            ("enter", b"\r"),
            ("up", b"\x1b[A"),
            ("down", b"\x1b[B"),
            ("q", b"q"),
            ("tab", b"\t"),
            ("esc", b"\x1b"),
        ] {
            let done = call(
                window,
                cx,
                "m1",
                "terminals.send",
                json!({"id": id, "key": key}),
            )
            .unwrap();
            assert_eq!(done["sent"], key);
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
    fn an_agent_cli_seen_live_or_by_the_poll_closes_the_terminal_for_good(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        send_key(window, cx, &id, "enter").unwrap();
        // An agent CLI that started since the last poll still gets nothing: asked live.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.foreground = None;
                    t.fake.front = program("claude");
                })
            })
            .unwrap();
        let err = send_key(window, cx, &id, "q").unwrap_err();
        assert!(err.contains("agent CLI"), "{err}");
        // The shell has the terminal again, but the conversation stays in its scrollback and
        // the terminal stays closed: not typed into, not listed, not read.
        set_front(window, cx, &t, Some(Front::Shell("zsh".into())));
        let err = send_key(window, cx, &id, "q").unwrap_err();
        assert!(err.contains("agent CLI"), "{err}");
        assert!(rows(window, cx).is_empty());
        let err = call(window, cx, "m1", "terminals.read", json!({"id": id})).unwrap_err();
        assert!(err.contains("agent CLI"), "{err}");
        assert_eq!(sent(window, cx, &id), b"\r");
        // It can still be stopped and closed: that only ends what the Mono started.
        call(window, cx, "m1", "terminals.close", json!({"id": id})).unwrap();
        // The poll alone counts too: one sighting is enough.
        let other = dock(window, cx, "FE");
        let o = terminal(window, cx, &other);
        window
            .update(cx, |_, _, cx| {
                o.update(cx, |t, _| t.foreground = Some("codex".into()))
            })
            .unwrap();
        assert!(send_key(window, cx, &other, "q").is_err());
        window
            .update(cx, |_, _, cx| o.update(cx, |t, _| t.foreground = None))
            .unwrap();
        assert!(send_key(window, cx, &other, "q").is_err());
    }

    #[gpui::test]
    fn a_terminal_that_cannot_be_asked_is_not_typed_into_where_the_system_can_look(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        set_front(window, cx, &t, Some(Front::Unknown));
        let attempt = send_key(window, cx, &id, "q");
        if CAN_INSPECT {
            assert!(attempt.unwrap_err().contains("cannot tell"));
            assert!(sent(window, cx, &id).is_empty());
        } else {
            attempt.unwrap();
        }
        // A group that cannot be listed is the same.
        set_front(window, cx, &t, None);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.fake.group_unknown = true)
            })
            .unwrap();
        assert_eq!(send_key(window, cx, &id, "q").is_ok(), !CAN_INSPECT);
    }

    #[gpui::test]
    fn every_process_in_front_must_be_inside_the_project_to_be_typed_into(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        // `true | (cd / && exec zsh -i)`: the leader is inside, the shell that has the keyboard
        // is not (and `sudo -s` would hide its folder altogether).
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.fake.group = Some(vec![
                        Member {
                            pid: 10,
                            args: "sleep 1000000000".into(),
                            cwd: Some(PROJECT.into()),
                        },
                        Member {
                            pid: 11,
                            args: "zsh -i".into(),
                            cwd: Some("/".into()),
                        },
                    ]);
                })
            })
            .unwrap();
        let err = send_key(window, cx, &id, "enter").unwrap_err();
        assert!(err.contains("outside the project") || !CAN_INSPECT, "{err}");
        // An agent piped into: the group holds it.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.fake.group = Some(vec![
                        Member {
                            pid: 10,
                            args: "cat prompt.txt".into(),
                            cwd: Some(PROJECT.into()),
                        },
                        Member {
                            pid: 11,
                            args: "claude".into(),
                            cwd: Some(PROJECT.into()),
                        },
                    ]);
                })
            })
            .unwrap();
        assert!(
            send_key(window, cx, &id, "enter")
                .unwrap_err()
                .contains("agent CLI")
        );
        assert!(sent(window, cx, &id).is_empty());
    }

    #[gpui::test]
    fn an_unknown_foreground_is_not_reused_and_a_busy_one_neither(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let first = dock(window, cx, "BE");
        let t = terminal(window, cx, &first);
        // `ps` failed or this is Windows: the live answer is "unknown". Reusing types a
        // command, which needs the same answers a write does, so a new terminal opens.
        set_front(window, cx, &t, Some(Front::Unknown));
        let again = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "place": "dock"}),
        )
        .unwrap();
        assert_eq!(again["reused"], false, "{again}");
        assert_eq!(rows(window, cx).len(), 2);
        // The last poll saw a program: not the same idle terminal, so a new one opens.
        let second = terminal(window, cx, again["id"].as_str().unwrap());
        for terminal in [&t, &second] {
            window
                .update(cx, |_, _, cx| {
                    terminal.update(cx, |t, _| {
                        t.fake.front = None;
                        t.foreground = Some("sleep".into())
                    })
                })
                .unwrap();
        }
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
    fn a_users_terminal_is_read_without_a_switch_but_never_driven(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let user = user_terminal(window, cx);
        window
            .update(cx, |_, _, cx| {
                user.update(cx, |t, cx| t.fake_output("user output\r\n", cx))
            })
            .unwrap();
        let id = rows(window, cx)[0]["id"].as_str().unwrap().to_owned();
        assert_eq!(rows(window, cx)[0]["owner"], "user");
        for (action, input) in [
            ("terminals.send", json!({"id": id, "key": "enter"})),
            ("terminals.send", json!({"id": id, "text": "ls"})),
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
        // Reading needs no per-terminal switch any more.
        let read = call(window, cx, "m1", "terminals.read", json!({"id": id})).unwrap();
        assert!(read["text"].as_str().unwrap().contains("user output"));
        // Another Mono's terminal is read-only to this one.
        let theirs = call(
            window,
            cx,
            "m2",
            "terminals.open",
            json!({"label": "T", "place": "dock"}),
        )
        .unwrap();
        call(
            window,
            cx,
            "m1",
            "terminals.read",
            json!({"id": theirs["id"]}),
        )
        .unwrap();
    }

    #[gpui::test]
    fn a_terminal_that_is_not_a_plain_shell_or_runs_an_agent_is_never_listed_or_read(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        let user = user_terminal(window, cx);
        let agent = user_terminal(window, cx);
        let launched = user_terminal(window, cx);
        window
            .update(cx, |_, _, cx| {
                agent.update(cx, |t, _| t.foreground = Some("codex".into()));
                launched.update(cx, |t, _| t.pretend_launched());
            })
            .unwrap();
        let list = rows(window, cx);
        assert_eq!(list.len(), 1, "{list:?}");
        let id = list[0]["id"].as_str().unwrap();
        assert!(call(window, cx, "m1", "terminals.read", json!({"id": id})).is_ok());
        // An id the registry knows for a terminal that turned into an agent session since.
        window
            .update(cx, |_, _, cx| {
                user.update(cx, |t, _| t.foreground = Some("claude".into()))
            })
            .unwrap();
        let err = call(window, cx, "m1", "terminals.read", json!({"id": id})).unwrap_err();
        assert!(err.contains("agent CLI"), "{err}");
        assert!(rows(window, cx).is_empty());
    }

    #[gpui::test]
    fn a_terminal_outside_the_project_gets_nothing_at_write_time(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        // The live folder is asked first: a `cd` out of the project closes the terminal to the
        // Mono, for text and for keys alike.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| t.fake.cwd = Some("/tmp".into()))
            })
            .unwrap();
        for input in [
            json!({"id": id, "text": "ls"}),
            json!({"id": id, "key": "enter"}),
        ] {
            let err = call(window, cx, "m1", "terminals.send", input).unwrap_err();
            assert!(err.contains("outside the project"), "{err}");
        }
        assert!(sent(window, cx, &id).is_empty());
        // Back inside (a subfolder), the same call goes through.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.fake.cwd = Some(format!("{PROJECT}/packages/api"))
                })
            })
            .unwrap();
        call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls"}),
        )
        .unwrap();
        // A terminal the test PTY places nowhere is judged where it started.
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, _| {
                    t.fake.cwd = None;
                    t.cwd = "/tmp".into();
                })
            })
            .unwrap();
        let err = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "key": "q"}),
        )
        .unwrap_err();
        assert!(err.contains("outside the project"), "{err}");
        assert_eq!(sent(window, cx, &id), b"ls");
        // Stop and close still work: they only end what the Mono started.
        call(window, cx, "m1", "terminals.stop", json!({"id": id})).unwrap();
        call(window, cx, "m1", "terminals.close", json!({"id": id})).unwrap();
    }

    #[gpui::test]
    fn input_over_4_kib_or_with_control_bytes_is_refused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        t_bracketed(window, cx, &id);
        let fits = "x".repeat(4096);
        for (action, field) in [("terminals.send", "text"), ("terminals.open", "command")] {
            let build = |value: &str| {
                if field == "text" {
                    json!({"id": id, "text": value})
                } else {
                    json!({"label": "BE2", "command": value})
                }
            };
            for bad in ["x".repeat(4097), "\u{e9}".repeat(2049)] {
                let err = call(window, cx, "m1", action, build(&bad)).unwrap_err();
                assert!(err.contains("4096"), "{action}: {err}");
            }
            let err = call(window, cx, "m1", action, build("")).unwrap_err();
            assert!(err.contains("empty"), "{err}");
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
            // The largest allowed input goes through.
            call(window, cx, "m1", action, build(&fits)).unwrap();
        }
        assert_eq!(sent(window, cx, &id).len(), 4096);
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

    /// Turns bracketed paste on in terminal `id`, as a shell at its prompt does.
    fn t_bracketed(window: WindowHandle<Shell>, cx: &mut TestAppContext, id: &str) {
        let t = terminal(window, cx, id);
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| t.fake_output("\x1b[?2004h", cx))
            })
            .unwrap();
    }

    #[gpui::test]
    fn multi_line_text_is_a_bracketed_paste_and_refused_when_the_program_did_not_ask_for_one(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = window(cx);
        let id = dock(window, cx, "BE");
        let t = terminal(window, cx, &id);
        // Off: each line would run as it arrives, so it is refused.
        let err = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "a\nb"}),
        )
        .unwrap_err();
        assert!(err.contains("bracketed paste"), "{err}");
        assert!(sent(window, cx, &id).is_empty());
        t_bracketed(window, cx, &id);
        call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "c\nd"}),
        )
        .unwrap();
        assert_eq!(sent(window, cx, &id), b"\x1b[200~c\nd\x1b[201~");
        // One line stays bare in both modes, and Enter can follow in the same call.
        let before = sent(window, cx, &id).len();
        let result = call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "ls -la", "key": "enter"}),
        )
        .unwrap();
        assert_eq!(result["key"], "enter");
        assert_eq!(sent(window, cx, &id)[before..], *b"ls -la\r");
        window
            .update(cx, |_, _, cx| {
                t.update(cx, |t, cx| t.fake_output("\x1b[?2004l", cx))
            })
            .unwrap();
        let before = sent(window, cx, &id).len();
        call(
            window,
            cx,
            "m1",
            "terminals.send",
            json!({"id": id, "text": "q"}),
        )
        .unwrap();
        assert_eq!(sent(window, cx, &id)[before..], *b"q");
    }

    #[gpui::test]
    fn a_multi_line_command_for_a_new_terminal_is_refused(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let err = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "BE", "command": "echo a\necho b"}),
        )
        .unwrap_err();
        assert!(err.contains("bracketed paste"), "{err}");
        assert_eq!(all_terminals(window, cx), 0);
    }

    #[gpui::test]
    fn a_saved_action_and_a_free_command_run_without_asking(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
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
        assert_eq!(
            sent(window, cx, opened["id"].as_str().unwrap()),
            b"npm run dev\r"
        );
        let id = open(
            window,
            cx,
            json!({"label": "FE", "command": "npm run web", "place": "dock"}),
        );
        assert_eq!(sent(window, cx, &id), b"npm run web\r");
    }

    #[gpui::test]
    fn open_places_a_terminal_in_the_dock_in_a_title_tab_or_in_a_split(cx: &mut TestAppContext) {
        let (window, chat) = logs_setup(cx);
        // dock: a tab of the project's dock.
        let dock_t = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "DOCK", "place": "dock", "reveal": false}),
        )
        .unwrap();
        assert_eq!(dock_t["place"], "dock");
        // tab: its own title tab, even though the user's default is the title placement.
        let tab_t = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "TAB", "place": "tab", "reveal": false}),
        )
        .unwrap();
        assert_eq!(tab_t["place"], "tab");
        let tab_pane = pane_of(window, cx, tab_t["id"].as_str().unwrap());
        // split: beside the terminal it names...
        let split_t = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "SPLIT", "place": "split", "beside": tab_t["id"], "reveal": false}),
        )
        .unwrap();
        assert_eq!(split_t["place"], "split");
        // ...and a split without `beside` goes into the Logs tab, never into the plain tab.
        let logs_t = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "LOGS", "place": "split", "reveal": false}),
        )
        .unwrap();
        assert_eq!(
            logs_t["place"], "tab",
            "a Logs tab of its own to start with"
        );
        window
            .update(cx, |shell, _, _| {
                let tab = shell.workspace.tab_of(tab_pane).unwrap();
                assert_eq!(shell.workspace.layout(tab).leaf_ids().len(), 2);
                // The plain tab has no Logs identity; the Logs tab has.
                assert_eq!(shell.workspace.terminal_panes[&tab_pane].logs, None);
                let logs = pane_of_state(shell, logs_t["id"].as_str().unwrap());
                assert_ne!(shell.workspace.tab_of(logs), Some(tab));
                // The user's chat tab was never split.
                let chat_tab = shell.workspace.tab_of(chat).unwrap();
                assert_eq!(shell.workspace.layout(chat_tab).leaf_ids(), vec![chat]);
            })
            .unwrap();
        // Nothing but the three allowed values.
        for bad in [json!("floating"), json!(3), json!("")] {
            let err = call(
                window,
                cx,
                "m1",
                "terminals.open",
                json!({"label": "X", "place": bad}),
            )
            .unwrap_err();
            assert!(err.contains("\"dock\", \"tab\" or \"split\""), "{err}");
        }
        // `beside` belongs to a split.
        let err = call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "X", "place": "tab", "beside": tab_t["id"]}),
        )
        .unwrap_err();
        assert!(err.contains("beside"), "{err}");
        assert_eq!(rows(window, cx).len(), 4);
    }

    /// The pane of a terminal, from the shell's own state.
    fn pane_of_state(shell: &Shell, id: &str) -> i64 {
        let entry = shell
            .mono_registry
            .entries
            .iter()
            .find(|e| e.id == id)
            .unwrap();
        let terminal = entry.terminal.upgrade().unwrap();
        *shell
            .terminal_pane_views
            .iter()
            .find(|(_, v)| **v == terminal)
            .unwrap()
            .0
    }

    #[gpui::test]
    fn layout_arranges_dock_tab_and_split_terminals_together(cx: &mut TestAppContext) {
        let (window, _chat) = logs_setup(cx);
        let dock_id = dock(window, cx, "DOCK");
        let tab_id = open(
            window,
            cx,
            json!({"label": "TAB", "place": "tab", "reveal": false}),
        );
        let first = open(
            window,
            cx,
            json!({"label": "S1", "place": "split", "reveal": false}),
        );
        let second = open(
            window,
            cx,
            json!({"label": "S2", "place": "split", "beside": first, "reveal": false}),
        );
        let done = layout(window, cx, "grid", &[&dock_id, &tab_id, &first, &second]).unwrap();
        assert_eq!(done["tab"], "Logs monocode");
        window
            .update(cx, |shell, _, _| {
                // The dock terminal left the dock; all four are panes of one tab.
                assert!(
                    shell
                        .workspace
                        .docks
                        .get(PROJECT)
                        .is_none_or(|d| d.terminals.is_empty())
                );
                let panes: Vec<_> = [&dock_id, &tab_id, &first, &second]
                    .iter()
                    .map(|id| pane_of_state(shell, id))
                    .collect();
                let tab = shell.workspace.tab_of(panes[0]).unwrap();
                assert!(
                    panes
                        .iter()
                        .all(|p| shell.workspace.tab_of(*p) == Some(tab))
                );
                assert_eq!(shell.workspace.layout(tab).leaf_ids().len(), 4);
                assert_eq!(shell.workspace.logs_title(tab), Some("Logs monocode"));
            })
            .unwrap();
        let list = rows(window, cx);
        assert_eq!(list.len(), 4);
        assert!(list.iter().all(|r| r["place"] == "split"));
    }

    #[gpui::test]
    fn a_restart_brings_back_no_mono_terminal_and_keeps_the_users(cx: &mut TestAppContext) {
        let (window, _chat) = logs_setup(cx);
        // The user's dock tab is in its own folder, so it cannot be mistaken for a Mono's.
        let user_cwd = format!("{PROJECT}/api");
        window
            .update(cx, |shell, _, cx| {
                let t = shell.spawn_terminal(user_cwd.clone(), cx);
                shell.terminals.entry(PROJECT.into()).or_default().push(t);
                shell
                    .workspace
                    .docks
                    .entry(PROJECT.into())
                    .or_default()
                    .add(user_cwd.clone());
            })
            .unwrap();
        let be = dock(window, cx, "BE");
        let tab = open(
            window,
            cx,
            json!({"label": "TAB", "place": "tab", "reveal": false}),
        );
        let fe = open(
            window,
            cx,
            json!({"label": "FE", "place": "split", "reveal": false}),
        );
        let before: Vec<_> = rows(window, cx)
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect();
        for id in [&be, &tab, &fe] {
            assert!(before.contains(id), "{id} not in {before:?}");
        }
        let saved = saved(window, cx);
        assert!(!saved.contains("\"mono\""), "{saved}");
        assert!(!saved.contains("BE") && !saved.contains("FE"), "{saved}");
        let window = restarted(cx, saved);
        window
            .update(cx, |shell, _, cx| {
                // Only the user's own dock tab came back, and no pane at all.
                assert_eq!(
                    shell.workspace.docks[PROJECT].terminals,
                    std::slice::from_ref(&user_cwd)
                );
                assert!(shell.workspace.terminal_panes.is_empty());
                assert!(shell.workspace.tabs.iter().all(|tab| {
                    shell
                        .workspace
                        .layout(*tab)
                        .leaf_ids()
                        .iter()
                        .all(|id| *id >= 0)
                }));
                // Shown, the dock reopens that one tab as a fresh shell.
                shell.ensure_terminals(PROJECT, cx);
            })
            .unwrap();
        let after = rows(window, cx);
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(after[0]["cwd"], user_cwd.as_str());
        assert_eq!(after[0]["owner"], "user");
        // No id survives: the ids of the earlier run name nothing, the Mono's among them.
        for id in [&be, &tab, &fe] {
            let err = call(window, cx, "m1", "terminals.read", json!({"id": id})).unwrap_err();
            assert!(err.contains("Unknown terminal"), "{id}: {err}");
        }
    }

    #[gpui::test]
    fn a_session_state_saved_with_mono_panes_and_tabs_loads_without_them(cx: &mut TestAppContext) {
        // The old format: a dock with `mono_terminals` and terminal panes with mono, label, run
        // and logs, each in a title tab of its own.
        let (window, chat) = logs_setup(cx);
        let user = user_terminal(window, cx);
        let _ = user;
        let current = saved(window, cx);
        let mut state: Value = serde_json::from_str(&current).unwrap();
        state["docks"][PROJECT]["terminals"] = json!([PROJECT, PROJECT, PROJECT]);
        state["docks"][PROJECT]["mono_terminals"] = json!([
            null,
            {"mono": "m1", "label": "BE", "run": {"kind": "command", "text": "npm run dev"}},
            null
        ]);
        state["docks"][PROJECT]["active"] = json!(1);
        state["terminal_panes"] = json!({
            "-1": {"owner": chat, "cwd": PROJECT, "mono": "m1", "label": "FE",
                   "run": {"kind": "action", "text": "npm run web", "action": "a1"},
                   "logs": "Logs monocode"},
            "-2": {"owner": chat, "cwd": PROJECT}
        });
        let tab = window
            .update(cx, |shell, _, _| shell.workspace.tab_of(chat).unwrap())
            .unwrap();
        state["tabs"] = json!([tab, -1, -2]);
        let window = restarted(cx, state.to_string());
        window
            .update(cx, |shell, _, _| {
                // The Mono's pane is gone; the user's own pane (no mono) is kept.
                assert!(!shell.workspace.terminal_panes.contains_key(&-1));
                assert!(shell.workspace.terminal_panes.contains_key(&-2));
                assert!(shell.workspace.tab_of(-1).is_none());
                // The dock lost the Mono's tab and kept the other two.
                let dock = &shell.workspace.docks[PROJECT];
                assert_eq!(dock.terminals.len(), 2);
                assert_eq!(dock.active, 1);
                assert!(!dock.is_mono(0) && !dock.is_mono(1));
            })
            .unwrap();
    }

    /// The call without the window: what it left behind (the `mono_switched` flag) can be
    /// read before `settle_mono_focus` gives the keyboard away.
    fn call_only(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        input: Value,
    ) -> Result<Value, String> {
        window
            .update(cx, |shell, _, cx| {
                shell.mono_terminals_call("m1", PROJECT, "terminals.open", &input, cx)
            })
            .unwrap()
    }

    fn switched(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |shell, _, _| std::mem::take(&mut shell.mono_switched))
            .unwrap()
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
                        cx,
                    )
                })
                .unwrap()
        };
        let error = open_in(cx, json!(outside.to_string_lossy())).unwrap_err();
        assert!(error.contains("must be inside the project"), "{error}");
        let error = open_in(cx, json!("../other")).unwrap_err();
        assert!(error.contains(".."), "{error}");
        let error = open_in(cx, json!("sub/../../other")).unwrap_err();
        assert!(error.contains(".."), "{error}");
        let error = open_in(cx, json!("")).unwrap_err();
        assert!(error.contains("must not be empty"), "{error}");
        let error = open_in(cx, json!(3)).unwrap_err();
        assert!(error.contains("cwd must be text"), "{error}");
        assert_eq!(all_terminals(window, cx), 0);
        // The same call with a folder inside the project goes through.
        let done = open_in(cx, json!("sub")).unwrap();
        assert_eq!(done["reused"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui::test]
    fn a_project_spelled_with_a_trailing_slash_is_the_same_project(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let spelled = format!("{PROJECT}/");
        let open_spelled = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, cx| {
                    shell.mono_terminals_call(
                        "m1",
                        &spelled,
                        "terminals.open",
                        &json!({"label": "BE", "place": "dock", "reveal": false}),
                        cx,
                    )
                })
                .unwrap()
                .unwrap()
        };
        let opened = open_spelled(cx);
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
        let again = open_spelled(cx);
        assert_eq!(
            (again["reused"].clone(), again["id"].clone()),
            (json!(true), opened["id"].clone())
        );
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
            let done = call(
                window,
                cx,
                "m1",
                "terminals.open",
                json!({"label": label, "place": "split", "reveal": true}),
            )
            .unwrap();
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
    fn the_keyboard_goes_to_the_shell_root_when_a_call_selects_the_monos_tab(
        cx: &mut TestAppContext,
    ) {
        let (window, chat) = logs_setup(cx);
        let user = user_terminal(window, cx);
        let focus = |cx: &mut TestAppContext| window.update(cx, |_, w, cx| w.focused(cx)).unwrap();
        window
            .update(cx, |_, w, cx| w.focus(&user.focus_handle(cx), cx))
            .unwrap();
        assert!(focus(cx).is_some());
        call(
            window,
            cx,
            "m1",
            "terminals.open",
            json!({"label": "FE", "command": "ls", "place": "split", "reveal": true}),
        )
        .unwrap();
        // The call selected the Mono's tab, which hides what had the keyboard: the shell
        // root takes it, never the terminal.
        let root = window
            .update(cx, |shell, _, _| shell.focus.clone())
            .unwrap();
        assert!(focus(cx) == Some(root));
        window
            .update(cx, |shell, _, _| {
                assert_ne!(shell.workspace.active, Some(chat));
                assert!(!shell.mono_switched);
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_list_and_read_of_a_mono_cover_the_project_not_the_cwd(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let user = user_terminal(window, cx);
        // A user terminal that `cd`ed elsewhere is still the project's to read.
        window
            .update(cx, |_, _, cx| {
                user.update(cx, |t, cx| {
                    t.fake.cwd = Some("/tmp".into());
                    t.fake_output("elsewhere\r\n", cx)
                })
            })
            .unwrap();
        let list = rows(window, cx);
        assert_eq!(list.len(), 1);
        let id = list[0]["id"].as_str().unwrap();
        let read = call(window, cx, "m1", "terminals.read", json!({"id": id})).unwrap();
        assert!(read["text"].as_str().unwrap().contains("elsewhere"));
    }
}
