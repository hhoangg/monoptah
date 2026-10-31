//! Preflight: does this machine have what a workflow declares in `requires`?
//!
//! Requirements are only ever looked up by name, never run, and an environment variable's value
//! is never read into anything that is kept: only whether it is set. The checks go through
//! `PreflightProbe` so tests never reach a real CLI, login shell or MCP server.
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use super::model::Workflow;
use super::model::{
    with_run_values, InputType, InputValue, NodeKind, Requirement, RequirementKind,
};
use super::ordered::{Json, OrderedMap};
use super::validate::{is_cli_name, is_env_name};
use crate::mcp::McpConnection;

/// How long `native_preflight` waits for all checks together.
pub const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(12);
/// How long `claude mcp list` may take across all the folders asked about.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the login shell may take to answer which variables it sets.
const LOGIN_ENV_TIMEOUT_MS: u64 = 5000;

/// (server name, status) pairs from `claude mcp list`.
type ServerStatuses = Vec<(String, String)>;

const NOT_IN_TIME: &str = "Could not check in time; try again";
const COMMAND_ONLY: &str = "Set for command steps only; session steps do not receive it";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequirementStatus {
    pub requirement: Requirement,
    pub met: bool,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preflight {
    pub statuses: Vec<RequirementStatus>,
}

impl Preflight {
    pub fn all_met(&self) -> bool {
        self.statuses.iter().all(|s| s.met)
    }

    pub fn missing(&self) -> Vec<&RequirementStatus> {
        self.statuses.iter().filter(|s| !s.met).collect()
    }

    /// One line naming what is missing, e.g. `cli gh: not found on PATH (brew install gh); env X:
    /// not set`. Empty when everything is met.
    pub fn summary(&self) -> String {
        self.missing()
            .iter()
            .map(|s| {
                let detail = s.detail.as_deref().unwrap_or("not met");
                let mut chars = detail.chars();
                let detail: String = match chars.next() {
                    Some(c) => c.to_lowercase().chain(chars).collect(),
                    None => String::new(),
                };
                let mut line = format!(
                    "{} {}: {}",
                    s.requirement.kind.key(),
                    s.requirement.name,
                    detail
                );
                if let Some(hint) = &s.requirement.hint {
                    line.push_str(&format!(" ({hint})"));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// `[{"kind","name","detail","hint"}]` for what is missing; `hint` only when the file has one.
    pub fn missing_json(&self) -> Json {
        Json::Array(
            self.missing()
                .iter()
                .map(|s| {
                    let mut o = OrderedMap::new();
                    o.insert("kind", Json::String(s.requirement.kind.key().into()));
                    o.insert("name", Json::String(s.requirement.name.clone()));
                    o.insert(
                        "detail",
                        Json::String(s.detail.clone().unwrap_or_else(|| "Not met".into())),
                    );
                    if let Some(hint) = &s.requirement.hint {
                        o.insert("hint", Json::String(hint.clone()));
                    }
                    Json::Object(o)
                })
                .collect(),
        )
    }
}

/// What the checks ask of the machine. Every method may block; call off the UI thread.
pub trait PreflightProbe {
    fn cli_found(&self, name: &str) -> bool;
    /// Set and non-empty in this process.
    fn env_process(&self, name: &str) -> bool;
    /// The names, out of `names`, that the login shell sets. None: could not tell.
    fn env_login(&self, names: &[String]) -> Option<Vec<String>>;
    fn mcp_servers(&self, cwds: &[String]) -> Result<Vec<McpConnection>, String>;
    /// `claude mcp list` as seen from `cwd`: (server name, status) pairs, with the statuses
    /// `mcp::parse_claude_health` produces. None: could not tell in time.
    fn mcp_statuses(&self, cwd: &str) -> Option<ServerStatuses>;
}

fn status(req: &Requirement, met: bool, detail: Option<&str>) -> RequirementStatus {
    RequirementStatus {
        requirement: req.clone(),
        met,
        detail: detail.map(str::to_string),
    }
}

/// Absolute project paths the workflow's `project` inputs hold, then the home folder.
fn mcp_cwds(w: &Workflow) -> Vec<String> {
    let w = with_run_values(w);
    let mut out: Vec<String> = Vec::new();
    for (name, input) in w.inputs.iter() {
        if input.ty != InputType::Project {
            continue;
        }
        if let Some(InputValue::String(path)) = w.input_values.get(name) {
            if Path::new(path).is_absolute() && !out.contains(path) {
                out.push(path.clone());
            }
        }
    }
    if let Some(home) = crate::scm::shell::dirs_home() {
        if !out.contains(&home) {
            out.push(home);
        }
    }
    out
}

pub fn evaluate(w: &Workflow, probe: &impl PreflightProbe) -> Preflight {
    let mut slots: Vec<Option<RequirementStatus>> = vec![None; w.requires.len()];
    evaluate_streaming(w, probe, &mut |i, st| slots[i] = Some(st));
    Preflight {
        statuses: slots.into_iter().flatten().collect(),
    }
}

/// Decides cli requirements first, then env, then mcp, handing each status to `sink` (with the
/// index into `w.requires`) as soon as it is known.
fn evaluate_streaming(
    w: &Workflow,
    probe: &impl PreflightProbe,
    sink: &mut dyn FnMut(usize, RequirementStatus),
) {
    let has_command = w
        .nodes
        .iter()
        .any(|n| matches!(n.kind, NodeKind::Command { .. }));
    let has_session = w
        .nodes
        .iter()
        .any(|n| matches!(n.kind, NodeKind::Session { .. }));
    let of_kind = |kind: RequirementKind| {
        w.requires
            .iter()
            .enumerate()
            .filter(move |(_, r)| r.kind == kind)
    };

    for (i, req) in of_kind(RequirementKind::Cli) {
        let s = if !is_cli_name(&req.name) {
            status(req, false, Some("Invalid name"))
        } else if probe.cli_found(&req.name) {
            status(req, true, None)
        } else {
            status(req, false, Some("Not found on PATH"))
        };
        sink(i, s);
    }

    // One login-shell question for every variable the process lacks, only when a step that
    // sources the login profile (a command step) could be what satisfies it.
    let mut login: Option<Vec<String>> = None;
    if has_command || !has_session {
        let unset: Vec<String> = of_kind(RequirementKind::Env)
            .map(|(_, r)| r)
            .filter(|r| is_env_name(&r.name) && !probe.env_process(&r.name))
            .map(|r| r.name.clone())
            .collect();
        if !unset.is_empty() {
            login = probe.env_login(&unset);
        }
    }
    for (i, req) in of_kind(RequirementKind::Env) {
        let s = if !is_env_name(&req.name) {
            status(req, false, Some("Invalid name"))
        } else {
            let process = probe.env_process(&req.name);
            let reach_session = process;
            let reach_command = process
                || login
                    .as_ref()
                    .is_some_and(|names| names.iter().any(|n| n == &req.name));
            let met = match (has_command, has_session) {
                (false, false) => reach_command,
                (c, s) => (!c || reach_command) && (!s || reach_session),
            };
            if met {
                status(req, true, None)
            } else if has_command && has_session && reach_command {
                status(req, false, Some(COMMAND_ONLY))
            } else {
                status(req, false, Some("Not set"))
            }
        };
        sink(i, s);
    }

    if of_kind(RequirementKind::Mcp).next().is_none() {
        return;
    }
    let cwds = mcp_cwds(w);
    let servers = probe.mcp_servers(&cwds);
    let mut health: Option<Vec<Option<ServerStatuses>>> = None;
    for (i, req) in of_kind(RequirementKind::Mcp) {
        let s = match &servers {
            // The discover error is not repeated: it can carry paths or config text.
            Err(_) => status(req, false, Some("Could not list MCP servers")),
            Ok(list) => {
                let found: Vec<&McpConnection> =
                    list.iter().filter(|c| c.name == req.name).collect();
                let enabled: Vec<&&McpConnection> = found.iter().filter(|c| c.enabled).collect();
                if found.is_empty() {
                    status(req, false, Some("Not configured"))
                } else if enabled.is_empty() {
                    status(req, false, Some("Disabled"))
                } else if enabled.iter().any(|c| c.provider != "claude") {
                    status(req, true, None)
                } else {
                    let seen = health.get_or_insert_with(|| {
                        cwds.iter().map(|cwd| probe.mcp_statuses(cwd)).collect()
                    });
                    let has = |wanted: &str| {
                        seen.iter()
                            .flatten()
                            .flatten()
                            .any(|(name, st)| name == &req.name && st == wanted)
                    };
                    if has("Needs authentication") {
                        status(req, false, Some("Configured but needs authentication"))
                    } else if has("Connected") {
                        status(req, true, None)
                    } else {
                        status(req, true, Some("Sign-in could not be confirmed"))
                    }
                }
            }
        };
        sink(i, s);
    }
}

/// Evaluates on a spawned thread and waits at most `timeout`. Each status is sent as it is
/// decided, so past the deadline only the requirements not decided yet are reported missing; the
/// thread may finish later and touches nothing shared.
pub(super) fn evaluate_bounded<P>(w: &Workflow, probe: Arc<P>, timeout: Duration) -> Preflight
where
    P: PreflightProbe + Send + Sync + 'static,
{
    let (tx, rx) = mpsc::channel();
    let owned = w.clone();
    std::thread::spawn(move || {
        evaluate_streaming(&owned, &*probe, &mut |i, st| {
            let _ = tx.send((i, st));
        });
    });
    let deadline = Instant::now() + timeout;
    let mut slots: Vec<Option<RequirementStatus>> = vec![None; w.requires.len()];
    while slots.iter().any(Option::is_none) {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((i, st)) => slots[i] = Some(st),
            Err(_) => break,
        }
    }
    Preflight {
        statuses: slots
            .into_iter()
            .zip(&w.requires)
            .map(|(known, r)| known.unwrap_or_else(|| status(r, false, Some(NOT_IN_TIME))))
            .collect(),
    }
}

/// How much earlier than the collector's deadline the probe stops its own work, so a status
/// decided at the clamp (kill, pipe join) still arrives before the collector gives up.
const PROBE_MARGIN: Duration = Duration::from_millis(500);

/// When the probe must be done, for a preflight that started at `start`.
pub(super) fn probe_deadline(start: Instant) -> Instant {
    start + PREFLIGHT_TIMEOUT.saturating_sub(PROBE_MARGIN)
}

/// The real checks, bounded overall. Returns at once when the workflow requires nothing.
pub fn native_preflight(w: &Workflow) -> Preflight {
    if w.requires.is_empty() {
        return Preflight {
            statuses: Vec::new(),
        };
    }
    evaluate_bounded(
        w,
        Arc::new(NativeProbe::new(probe_deadline(Instant::now()))),
        PREFLIGHT_TIMEOUT,
    )
}

/// `a`, then whatever `b` adds, one entry per kind and name. A hint `a` lacks is taken from `b`.
pub fn union_requires(a: &[Requirement], b: &[Requirement]) -> Vec<Requirement> {
    let mut out: Vec<Requirement> = Vec::new();
    for r in a.iter().chain(b) {
        match out
            .iter_mut()
            .find(|o| o.kind == r.kind && o.name == r.name)
        {
            Some(existing) => {
                if existing.hint.is_none() {
                    existing.hint = r.hint.clone();
                }
            }
            None => out.push(r.clone()),
        }
    }
    out
}

/// Words that start a shell construct or are built into the shell: never a program to install.
const SHELL_WORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "in", "function", "select", "time", "coproc", "source", "eval", "exec", "exit", "return",
    "set", "unset", "alias", "unalias", "read", "local", "declare", "typeset", "readonly", "shift",
    "trap", "wait", "type", "command", "builtin", "umask", "ulimit", "hash", "jobs", "fg", "bg",
    "kill", "getopts", "let", "break", "continue", "times",
];

/// Base utilities every POSIX system has, so naming them in `requires` would only be noise.
const BASE_UTILITIES: &[&str] = &[
    "sh", "bash", "ls", "cat", "echo", "grep", "sed", "awk", "cut", "sort", "head", "tail", "tr",
    "wc", "mkdir", "rm", "cp", "mv", "touch", "test", "true", "false", "printf", "find", "xargs",
    "date", "env", "sleep", "basename", "dirname", "pwd", "cd", "export",
];

/// The command line cut at `;`, newlines, `&&`, `||` and `|` that are not inside quotes.
fn command_segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == '\\' && q == '"' {
                    cur.extend(chars.next());
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '\\' => {
                    cur.push(c);
                    cur.extend(chars.next());
                }
                // A here-document body is data, not commands: ignore everything after it opens
                // (conservative, a later command on another line is missed rather than invented).
                '<' if chars.peek() == Some(&'<') => {
                    out.push(std::mem::take(&mut cur));
                    return out;
                }
                // A comment runs to the end of the line, but only when `#` starts a word.
                '#' if cur.is_empty() || cur.ends_with(char::is_whitespace) => {
                    while chars.peek().is_some_and(|&n| n != '\n') {
                        chars.next();
                    }
                }
                '\n' | ';' => out.push(std::mem::take(&mut cur)),
                '|' | '&' if chars.peek() == Some(&c) => {
                    chars.next();
                    out.push(std::mem::take(&mut cur));
                }
                '|' => out.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            },
        }
    }
    out.push(cur);
    out
}

/// The words of one segment, split on blanks outside quotes (the quotes stay in the word).
fn command_words(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in segment.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                cur.push(c);
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| is_env_name(name))
}

/// The programs a shell command probably runs, for the advice `workflows.validate` gives about
/// undeclared CLIs. Conservative on purpose: a segment with a template (`{{`), a command
/// substitution (`$(`) or a backtick is skipped whole, and only a plain command name that is not
/// a shell word or a base utility is kept. In order of first use, without repeats.
pub fn likely_clis(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for segment in command_segments(command) {
        if segment.contains("{{") || segment.contains("$(") || segment.contains('`') {
            continue;
        }
        let words = command_words(&segment);
        let Some(first) = words.iter().find(|w| !is_assignment(w)) else {
            continue;
        };
        if !is_cli_name(first)
            || SHELL_WORDS.contains(&first.as_str())
            || BASE_UTILITIES.contains(&first.as_str())
            || out.contains(first)
        {
            continue;
        }
        out.push(first.clone());
    }
    out
}

/// Prints each requested name whose variable is set and non-empty, never its value.
pub(crate) fn login_env_script(names: &[String]) -> String {
    names
        .iter()
        .filter(|n| is_env_name(n))
        .map(|n| format!("[ -n \"${{{n}-}}\" ] && printf '%s\\n' {n}\n"))
        .collect()
}

/// Only the lines that are exactly a requested name; anything else the shell printed is noise.
pub(crate) fn names_in_output(stdout: &str, names: &[String]) -> Vec<String> {
    stdout
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| names.iter().any(|n| n == l))
        .map(str::to_string)
        .collect()
}

struct NativeProbe {
    /// When the whole preflight gives up; the health check must end before it.
    deadline: Instant,
    /// When the first `claude mcp list` started; the health budget counts from there.
    health_started: Mutex<Option<Instant>>,
}

impl NativeProbe {
    fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            health_started: Mutex::new(None),
        }
    }
}

impl PreflightProbe for NativeProbe {
    fn cli_found(&self, name: &str) -> bool {
        crate::scm::shell::find_on_gui_path(name).is_some()
    }

    fn env_process(&self, name: &str) -> bool {
        std::env::var_os(name).is_some_and(|v| !v.is_empty())
    }

    fn env_login(&self, names: &[String]) -> Option<Vec<String>> {
        if cfg!(windows) {
            return None;
        }
        let names: Vec<String> = names.iter().filter(|n| is_env_name(n)).cloned().collect();
        if names.is_empty() {
            return Some(Vec::new());
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = format!(
            "preflight-env-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let home = crate::scm::shell::dirs_home()?;
        let result = super::command::run_command(
            &id,
            &login_env_script(&names),
            &home,
            LOGIN_ENV_TIMEOUT_MS,
        )
        .ok()?;
        if result.timed_out || result.cancelled || result.exit_code.is_none() {
            return None;
        }
        Some(names_in_output(&result.stdout, &names))
    }

    fn mcp_servers(&self, cwds: &[String]) -> Result<Vec<McpConnection>, String> {
        let mut all: Vec<McpConnection> = Vec::new();
        let mut first_error: Option<String> = None;
        let mut any_ok = false;
        for cwd in cwds {
            match crate::mcp::mcp_discover(cwd.clone()) {
                Ok(found) => {
                    any_ok = true;
                    for c in found {
                        let same = |o: &McpConnection| {
                            o.provider == c.provider
                                && o.name == c.name
                                && o.scope == c.scope
                                && o.config_path == c.config_path
                        };
                        if !all.iter().any(same) {
                            all.push(c);
                        }
                    }
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match (any_ok, first_error) {
            (false, Some(e)) => Err(e),
            _ => Ok(all),
        }
    }

    fn mcp_statuses(&self, cwd: &str) -> Option<Vec<(String, String)>> {
        let spent = {
            let mut started = self
                .health_started
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            started.get_or_insert_with(Instant::now).elapsed()
        };
        let budget = HEALTH_TIMEOUT.checked_sub(spent)?;
        let text = crate::mcp::claude_health_until(cwd, budget, self.deadline).ok()?;
        Some(crate::mcp::parse_claude_health(&text))
    }
}
