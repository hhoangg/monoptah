//! GitHub Codespaces control through the `gh` CLI.
//!
//! Every `gh` call goes through the injectable [`GhRunner`] so tests never
//! touch the network. Secret values only ever travel over stdin.
use std::collections::HashSet;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::cloud_sessions::backend::CloudBackendKind;
use serde::{Deserialize, Serialize};

pub const OAUTH_SECRET_KEY: &str = "CLAUDE_CODE_OAUTH_TOKEN";
pub const DEFAULT_MACHINE: &str = "basicLinux32gb";
pub const IDLE_TIMEOUT: &str = "30m";
/// [`IDLE_TIMEOUT`] in minutes, for the REST API.
pub const DEFAULT_IDLE_TIMEOUT_MINUTES: u32 = 30;
pub const RETENTION_PERIOD: &str = "72h";
pub const READY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const READY_POLL: Duration = Duration::from_secs(5);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const SLOW_CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// `gh codespace cp` may first have to wake the codespace, then move a directory.
const COPY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// A short `gh codespace ssh` setup command. Best-effort callers must never wait longer.
const SSH_TIMEOUT: Duration = Duration::from_secs(30);
/// A `gh codespace ssh` script marked with [`LONG_SCRIPT_MARKER`] (git bundles, large trees).
const LONG_SSH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Last argument of a `gh codespace ssh` call that may run longer than [`SSH_TIMEOUT`].
pub const LONG_SCRIPT_MARKER: &str = "monoptah-long";
const DISPLAY_NAME_MAX: usize = 48;
const GH_MISSING: &str = "GitHub CLI (gh) is not installed";
const SCOPE_HINT: &str = "The GitHub token is missing the `codespace` scope. \
Run `gh auth refresh -h github.com -s codespace` and try again.";

/// Captured result of one `gh` invocation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GhOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `gh` with the given arguments. `Err` means gh could not be started.
pub trait GhRunner: Send + Sync {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String>;

    /// Like `run`, but reports each stdout/stderr line as it arrives.
    fn run_streaming(
        &self,
        args: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<GhOutput, String> {
        let out = self.run(args, None)?;
        for line in out.stderr.lines().chain(out.stdout.lines()) {
            if !line.trim().is_empty() {
                on_line(line);
            }
        }
        Ok(out)
    }

    /// Like `run_streaming`, but a long-running command is killed as soon as `stop` becomes
    /// true (checked every ~200 ms); what was read so far is returned. The default ignores
    /// `stop`, which is right for fakes that return immediately.
    fn run_streaming_until(
        &self,
        args: &[String],
        on_line: &mut dyn FnMut(&str),
        _stop: &AtomicBool,
    ) -> Result<GhOutput, String> {
        self.run_streaming(args, on_line)
    }
}

/// Real runner: resolves `gh` on the login-shell PATH like the other CLIs.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemGh;

impl SystemGh {
    fn command(args: &[String]) -> Result<Command, String> {
        let program = crate::scm::resolve_gui_binary("gh").ok_or_else(|| GH_MISSING.to_string())?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_PAGER", "cat")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1");
        crate::scm::shell::apply_gui_env(&mut cmd);
        crate::process::configure_child(&mut cmd);
        Ok(cmd)
    }
}

fn spawn_error(error: std::io::Error) -> String {
    if error.kind() == ErrorKind::NotFound {
        GH_MISSING.to_string()
    } else {
        error.to_string()
    }
}

/// How long one non-streaming `gh` call may take before it is killed. Slow lifecycle
/// calls get longer; everything else (auth, listing, secrets) must answer quickly so a
/// keyring prompt or a dead network cannot hang a caller forever.
pub fn call_timeout(args: &[String]) -> Duration {
    if matches!(
        (
            args.first().map(String::as_str),
            args.get(1).map(String::as_str)
        ),
        (Some("codespace"), Some("cp"))
    ) {
        return COPY_TIMEOUT;
    }
    if matches!(
        (
            args.first().map(String::as_str),
            args.get(1).map(String::as_str)
        ),
        (Some("codespace"), Some("ssh"))
    ) {
        return if args.last().map(String::as_str) == Some(LONG_SCRIPT_MARKER) {
            LONG_SSH_TIMEOUT
        } else {
            SSH_TIMEOUT
        };
    }
    // The REST create (`gh api --method POST repos/<repo>/codespaces`) answers in seconds,
    // but its request may already have created a codespace when we give up. A premature
    // timeout would leave an orphan (and a retry a second one), so it gets the slow class
    // like `gh codespace create` instead of the 20 s default of other `gh api` reads.
    let rest_create = args.first().map(String::as_str) == Some("api")
        && args.iter().any(|a| a == "POST")
        && args.iter().any(|a| a.ends_with("/codespaces"));
    let slow = rest_create
        || matches!(
            (
                args.first().map(String::as_str),
                args.get(1).map(String::as_str)
            ),
            (
                Some("codespace"),
                Some("create" | "stop" | "delete" | "rebuild")
            )
        );
    if slow {
        SLOW_CALL_TIMEOUT
    } else {
        CALL_TIMEOUT
    }
}

impl SystemGh {
    fn run_with_timeout(
        args: &[String],
        stdin: Option<&str>,
        timeout: Duration,
    ) -> Result<GhOutput, String> {
        run_command(Self::command(args)?, args, stdin, timeout)
    }
}

/// Runs `cmd`, killing it when it outlives `timeout`.
fn run_command(
    mut cmd: Command,
    args: &[String],
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<GhOutput, String> {
    {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = cmd.spawn().map_err(spawn_error)?;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // A closed pipe means gh exited early; its status reports why.
            let _ = pipe.write_all(text.as_bytes());
        }
        let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut text = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut text);
                }
                String::from_utf8_lossy(&text).into_owned()
            })
        };
        let out = read(child.stdout.take().map(|p| Box::new(p) as _));
        let err = read(child.stderr.take().map(|p| Box::new(p) as _));
        let deadline = std::time::Instant::now() + timeout;
        let status = loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(status) => break status,
                None if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "GitHub CLI did not answer within {} seconds (`gh {}`). Check your network connection and that no keychain prompt is waiting.",
                        timeout.as_secs(),
                        args.first().map(String::as_str).unwrap_or("")
                    ));
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        Ok(GhOutput {
            success: status.success(),
            stdout: out.join().unwrap_or_default(),
            stderr: err.join().unwrap_or_default(),
        })
    }
}

impl GhRunner for SystemGh {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String> {
        Self::run_with_timeout(args, stdin, call_timeout(args))
    }

    fn run_streaming(
        &self,
        args: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<GhOutput, String> {
        Self::stream(args, on_line, None)
    }

    fn run_streaming_until(
        &self,
        args: &[String],
        on_line: &mut dyn FnMut(&str),
        stop: &AtomicBool,
    ) -> Result<GhOutput, String> {
        Self::stream(args, on_line, Some(stop))
    }
}

impl SystemGh {
    fn stream(
        args: &[String],
        on_line: &mut dyn FnMut(&str),
        stop: Option<&AtomicBool>,
    ) -> Result<GhOutput, String> {
        stream_command(Self::command(args)?, on_line, stop)
    }
}

/// Runs `cmd`, reporting its output lines; kills it (and its process group) when `stop` is set.
fn stream_command(
    mut cmd: Command,
    on_line: &mut dyn FnMut(&str),
    stop: Option<&AtomicBool>,
) -> Result<GhOutput, String> {
    {
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        // A stoppable command may have children (`gh codespace ssh` runs ssh): give it a
        // process group so the stop reaches all of them and the pipes close.
        #[cfg(unix)]
        if stop.is_some() {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().map_err(spawn_error)?;
        let (tx, rx) = mpsc::channel::<(bool, String)>();
        let mut readers = Vec::new();
        if let Some(out) = child.stdout.take() {
            let tx = tx.clone();
            readers.push(std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = tx.send((false, line));
                }
            }));
        }
        if let Some(err) = child.stderr.take() {
            let tx = tx.clone();
            readers.push(std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = tx.send((true, line));
                }
            }));
        }
        drop(tx);
        let (mut stdout, mut stderr) = (String::new(), String::new());
        let mut stopped = false;
        loop {
            let next = match stop {
                None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                Some(stop) => {
                    if stop.load(Ordering::Relaxed) {
                        stopped = true;
                        break;
                    }
                    rx.recv_timeout(Duration::from_millis(200))
                }
            };
            match next {
                Ok((is_err, line)) => {
                    if !line.trim().is_empty() && (!is_err || stop.is_none()) {
                        on_line(&line);
                    }
                    let buf = if is_err { &mut stderr } else { &mut stdout };
                    buf.push_str(&line);
                    buf.push('\n');
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if stopped {
            crate::process::terminate(&mut child);
        }
        for reader in readers {
            let _ = reader.join();
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        Ok(GhOutput {
            success: status.success() && !stopped,
            stdout,
            stderr,
        })
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

/// Turn a failed gh run into a user-facing message.
fn failure(args: &[String], out: &GhOutput) -> String {
    let stderr = out.stderr.trim();
    let detail = if !stderr.is_empty() {
        stderr
    } else {
        out.stdout.trim()
    };
    let lower = detail.to_lowercase();
    if lower.contains("scope") {
        return format!("{SCOPE_HINT} ({detail})");
    }
    if detail.is_empty() {
        format!(
            "gh {} failed",
            args.first().map(String::as_str).unwrap_or("")
        )
    } else {
        detail.to_string()
    }
}

fn run_ok(runner: &dyn GhRunner, args: Vec<String>) -> Result<String, String> {
    let out = runner.run(&args, None)?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(failure(&args, &out))
    }
}

fn run_json<T: for<'de> Deserialize<'de>>(
    runner: &dyn GhRunner,
    args: Vec<String>,
) -> Result<T, String> {
    let text = run_ok(runner, args)?;
    serde_json::from_str(&text).map_err(|e| format!("Unexpected gh output: {e}"))
}

/// Fetch every page of a REST listing and return the pages' objects.
fn api_pages(runner: &dyn GhRunner, path: &str) -> Result<Vec<serde_json::Value>, String> {
    let text = run_ok(runner, argv(&["api", "--paginate", "--slurp", path]))?;
    let mut pages = Vec::new();
    for value in serde_json::Deserializer::from_str(&text).into_iter::<serde_json::Value>() {
        match value.map_err(|e| format!("Unexpected gh output: {e}"))? {
            serde_json::Value::Array(items) => pages.extend(items),
            other => pages.push(other),
        }
    }
    Ok(pages)
}

fn collect_field(pages: &[serde_json::Value], list: &str, field: &str) -> Vec<String> {
    pages
        .iter()
        .filter_map(|page| page.get(list).and_then(|l| l.as_array()))
        .flatten()
        .filter_map(|item| item.get(field).and_then(|n| n.as_str()))
        .map(str::to_string)
        .collect()
}

/// `owner/repo` from an https, ssh, scp-like or git remote URL.
pub fn repo_from_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = if let Some((scheme, rest)) = url.split_once("://") {
        let rest = match scheme.to_ascii_lowercase().as_str() {
            "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git" => rest,
            _ => return None,
        };
        let (host, path) = rest.split_once('/')?;
        if host.rsplit('@').next().is_none_or(|h| h.is_empty()) {
            return None;
        }
        path.to_string()
    } else {
        // scp-like: [user@]host:owner/repo
        let (host, path) = url.split_once(':')?;
        if host.is_empty() || host.contains('/') {
            return None;
        }
        path.to_string()
    };
    let path = rest.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || !valid_segment(owner) || !valid_segment(repo) {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn valid_segment(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_repo(repo: &str) -> Result<(), String> {
    match repo.split_once('/') {
        Some((owner, name)) if valid_segment(owner) && valid_segment(name) => Ok(()),
        _ => Err(format!("Invalid repository `{repo}`, expected owner/repo")),
    }
}

fn valid_codespace_name(name: &str) -> Result<(), String> {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(())
    } else {
        Err(format!("Invalid codespace name `{name}`"))
    }
}

/// Repo path of the checkout inside a codespace.
pub fn workspace_path(repo: &str) -> Option<String> {
    let name = repo.split_once('/')?.1;
    Some(format!("/workspaces/{name}"))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub logged_in: bool,
    pub user: Option<String>,
    pub scopes: Vec<String>,
    pub has_codespace_scope: bool,
}

impl AuthStatus {
    /// Actionable error unless logged in with the `codespace` scope.
    pub fn require_codespace_scope(&self) -> Result<(), String> {
        if !self.logged_in {
            return Err("Not logged in to GitHub. Run `gh auth login` and try again.".into());
        }
        if !self.has_codespace_scope {
            return Err(SCOPE_HINT.into());
        }
        Ok(())
    }
}

/// Parse the text of `gh auth status` (stdout and stderr combined).
pub fn parse_auth_status(text: &str) -> AuthStatus {
    // Prefer the github.com block when several hosts are listed.
    let mut blocks: Vec<(String, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
            blocks.push((line.trim().to_string(), Vec::new()));
        } else if let Some((_, lines)) = blocks.last_mut() {
            lines.push(line);
        }
    }
    let lines: Vec<&str> = match blocks.iter().find(|(host, _)| host == "github.com") {
        Some((_, lines)) => lines.clone(),
        None => text.lines().collect(),
    };
    let mut status = AuthStatus::default();
    for line in lines {
        let line = line.trim().trim_start_matches(['-', '✓', 'X', '✗']).trim();
        if status.user.is_some() && line.starts_with("Logged in to") {
            // A second account: keep the first one's user and scopes together.
            break;
        }
        if status.user.is_none() {
            let account = line
                .split_once(" account ")
                .or_else(|| line.split_once(" as "))
                .filter(|_| line.starts_with("Logged in to"));
            if let Some((_, rest)) = account {
                let user = rest.split_whitespace().next().unwrap_or("");
                if !user.is_empty() {
                    status.logged_in = true;
                    status.user = Some(user.to_string());
                }
            }
        }
        if let Some(scopes) = line.strip_prefix("Token scopes:") {
            status.scopes = scopes
                .split(',')
                .map(|s| s.trim().trim_matches(['\'', '"']).to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    status.has_codespace_scope = status.scopes.iter().any(|s| s == "codespace");
    status
}

pub fn auth_status(runner: &dyn GhRunner) -> Result<AuthStatus, String> {
    let out = runner.run(&argv(&["auth", "status", "--active"]), None)?;
    let text = format!("{}\n{}", out.stdout, out.stderr);
    let mut status = parse_auth_status(&text);
    if !out.success && status.user.is_none() {
        status.logged_in = false;
    }
    Ok(status)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GitStatus {
    pub ahead: i64,
    pub behind: i64,
    pub has_uncommitted_changes: bool,
    pub has_unpushed_changes: bool,
    #[serde(rename = "ref")]
    pub git_ref: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Codespace {
    pub name: String,
    pub display_name: String,
    pub state: String,
    pub git_status: Option<GitStatus>,
    pub machine_name: String,
    pub repository: String,
    pub last_used_at: String,
}

const LIST_FIELDS: &str = "name,displayName,state,gitStatus,machineName,repository,lastUsedAt";

pub fn list(runner: &dyn GhRunner, repo: &str) -> Result<Vec<Codespace>, String> {
    valid_repo(repo)?;
    run_json(
        runner,
        argv(&["codespace", "list", "--repo", repo, "--json", LIST_FIELDS]),
    )
}

/// Default display name: `monoptah-<branch>`, at most 48 characters.
pub fn display_name_for(branch: &str) -> String {
    format!("monoptah-{branch}")
        .chars()
        .take(DISPLAY_NAME_MAX)
        .collect()
}

/// Optional settings of a new codespace; every `None` keeps today's default (machine
/// [`DEFAULT_MACHINE`], idle timeout [`IDLE_TIMEOUT`], the repository's default devcontainer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CreateOptions<'a> {
    pub machine: Option<&'a str>,
    /// Minutes before an idle codespace stops (GitHub allows 5 to 240).
    pub idle_timeout_minutes: Option<u32>,
    /// Devcontainer config path relative to the repository root.
    pub devcontainer_path: Option<&'a str>,
}

/// Lowest and highest idle timeout `gh` accepts, in minutes.
pub const IDLE_TIMEOUT_MINUTES: std::ops::RangeInclusive<u32> = 5..=240;
/// The message for an idle timeout outside [`IDLE_TIMEOUT_MINUTES`].
pub const IDLE_TIMEOUT_ERROR: &str = "Stop when idle must be between 5 and 240 minutes.";

impl<'a> CreateOptions<'a> {
    /// The machine type to request.
    fn machine(&self) -> &'a str {
        self.machine
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or(DEFAULT_MACHINE)
    }
    /// The idle timeout, when it differs from the default; rejects values gh refuses.
    fn idle(&self) -> Result<Option<u32>, String> {
        match self.idle_timeout_minutes {
            None => Ok(None),
            Some(m) if IDLE_TIMEOUT_MINUTES.contains(&m) => Ok(Some(m)),
            Some(_) => Err(IDLE_TIMEOUT_ERROR.into()),
        }
    }
    /// The devcontainer path to request, when set; rejects anything but a relative path.
    fn devcontainer(&self) -> Result<Option<&'a str>, String> {
        match self
            .devcontainer_path
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            None => Ok(None),
            Some(p) if crate::cloud_sessions::docker_config::valid_devcontainer_path(p) => {
                Ok(Some(p))
            }
            Some(_) => Err("The devcontainer path must be relative to the repository root".into()),
        }
    }
}

/// Create a codespace and return its name. Progress lines go to `on_progress`.
pub fn create(
    runner: &dyn GhRunner,
    repo: &str,
    branch: &str,
    options: &CreateOptions,
    display: Option<&str>,
    on_progress: &mut dyn FnMut(&str),
) -> Result<String, String> {
    valid_repo(repo)?;
    if branch.trim().is_empty() {
        return Err("A branch is required to create a codespace".into());
    }
    let machine = options.machine();
    let idle = options.idle()?.map(|m| format!("{m}m"));
    let devcontainer = options.devcontainer()?;
    let display = match display.filter(|d| !d.trim().is_empty()) {
        Some(d) => d.chars().take(DISPLAY_NAME_MAX).collect(),
        None => display_name_for(branch),
    };
    let mut args = argv(&[
        "codespace",
        "create",
        "--repo",
        repo,
        "--branch",
        branch,
        "--machine",
        machine,
        "--display-name",
        &display,
        "--idle-timeout",
        idle.as_deref().unwrap_or(IDLE_TIMEOUT),
        "--retention-period",
        RETENTION_PERIOD,
        "--default-permissions",
    ]);
    if let Some(path) = devcontainer {
        args.extend(argv(&["--devcontainer-path", path]));
    }
    let out = runner.run_streaming(&args, on_progress)?;
    if !out.success {
        return Err(failure(&args, &out));
    }
    out.stdout
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .filter(|name| valid_codespace_name(name).is_ok())
        .ok_or_else(|| "gh did not report the new codespace name".to_string())
}

/// A codespace whose creation was requested through the REST API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedCodespace {
    pub name: String,
    /// State at creation time, normally `Provisioning`.
    pub state: String,
    /// Browser URL (`https://<name>.github.dev`-style), when the API returned one.
    pub web_url: Option<String>,
}

/// Request a codespace through the REST API and return as soon as it exists.
///
/// Unlike [`create`] (`gh codespace create`), which polls for at most two minutes and prints
/// the name only at the very end (a slow devcontainer build fails it with "context deadline
/// exceeded" and the name is lost), the REST call answers in seconds with `201 Created` and
/// the codespace JSON (`name`, `state: Provisioning`, `web_url`). A `202` is possible too, so
/// any success with a valid `name` counts. Follow with [`wait_ready_with`] or poll
/// [`state`]. Defaults match [`create`]: machine [`DEFAULT_MACHINE`], display name
/// `monoptah-<branch>` (48 characters), idle timeout 30 minutes, retention 72 hours, and
/// `multi_repo_permissions_opt_out` (the equivalent of `--default-permissions`).
pub fn create_started(
    runner: &dyn GhRunner,
    repo: &str,
    branch: &str,
    options: &CreateOptions,
    display: Option<&str>,
) -> Result<StartedCodespace, String> {
    valid_repo(repo)?;
    if branch.trim().is_empty() {
        return Err("A branch is required to create a codespace".into());
    }
    let machine = options.machine();
    let idle = options.idle()?.unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES);
    let devcontainer = options.devcontainer()?;
    let display: String = match display.filter(|d| !d.trim().is_empty()) {
        Some(d) => d.chars().take(DISPLAY_NAME_MAX).collect(),
        None => display_name_for(branch),
    };
    let path = format!("repos/{repo}/codespaces");
    let mut args = argv(&[
        "api",
        "--method",
        "POST",
        &path,
        "-f",
        &format!("ref={branch}"),
        "-f",
        &format!("machine={machine}"),
        "-f",
        &format!("display_name={display}"),
        "-F",
        &format!("idle_timeout_minutes={idle}"),
        "-F",
        "retention_period_minutes=4320",
        "-F",
        "multi_repo_permissions_opt_out=true",
    ]);
    if let Some(path) = devcontainer {
        args.extend(argv(&["-f", &format!("devcontainer_path={path}")]));
    }
    let out = runner.run(&args, None)?;
    if !out.success {
        return Err(failure(&args, &out));
    }
    let value: serde_json::Value = serde_json::from_str(out.stdout.trim())
        .map_err(|e| format!("Unexpected gh output: {e}"))?;
    let name = value["name"]
        .as_str()
        .map(str::trim)
        .filter(|name| valid_codespace_name(name).is_ok())
        .ok_or_else(|| "GitHub did not report the new codespace name".to_string())?;
    let text = |key: &str| {
        value[key]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    Ok(StartedCodespace {
        name: name.to_string(),
        state: text("state").unwrap_or_else(|| "Provisioning".into()),
        web_url: text("web_url"),
    })
}

/// Browser URL of a codespace (`gh api user/codespaces/<name> --jq .web_url`).
pub fn web_url(runner: &dyn GhRunner, name: &str) -> Result<String, String> {
    valid_codespace_name(name)?;
    let path = format!("user/codespaces/{name}");
    let out = run_ok(runner, argv(&["api", &path, "--jq", ".web_url"]))?;
    let url = out.trim();
    if url.starts_with("https://") {
        Ok(url.to_string())
    } else {
        Err(format!("GitHub reported no web URL for codespace {name}"))
    }
}

/// Rebuild the codespace's container (`gh codespace rebuild`, no `--full`). It does not
/// prompt. The call returns once the rebuild is requested; poll [`state`] afterwards.
pub fn rebuild(runner: &dyn GhRunner, name: &str) -> Result<(), String> {
    valid_codespace_name(name)?;
    run_ok(runner, argv(&["codespace", "rebuild", "-c", name])).map(|_| ())
}

/// Where a codespace writes its creation log.
///
/// `gh codespace logs -c <name>` cannot be used (verified on gh 2.95.0): it waits for the
/// codespace to be Available, then runs plain `ssh` without the key gh installs and fails
/// with `Permission denied (publickey,password)`, and it would start a stopped codespace.
/// `gh codespace ssh -- tail ...` works once the codespace is Available; before that gh only
/// waits. The log is replayed from the top, so late followers still see real timestamps.
pub const CREATION_LOG: &str = "/workspaces/.codespaces/.persistedshare/creation.log";

/// Stream the creation log through `on_line` (raw lines, see [`parse_log_line`]) until `stop`
/// becomes true or the connection ends. Call only once the codespace is Available.
/// `Ok` also covers a stop; `Err` is a failed `gh` (not signed in, ssh refused).
pub fn follow_creation_log(
    runner: &dyn GhRunner,
    name: &str,
    stop: &AtomicBool,
    on_line: &mut dyn FnMut(&str),
) -> Result<(), String> {
    valid_codespace_name(name)?;
    let args = argv(&[
        "codespace",
        "ssh",
        "-c",
        name,
        "--",
        "tail",
        "-n",
        "+1",
        "-F",
        CREATION_LOG,
    ]);
    let out = runner.run_streaming_until(&args, on_line, stop)?;
    if out.success || stop.load(Ordering::Relaxed) {
        Ok(())
    } else {
        Err(failure(&args, &out))
    }
}

/// One line of the creation log, minus its timestamp prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    /// Epoch milliseconds from the `YYYY-MM-DD HH:MM:SS.mmmZ: ` prefix. `None` for a
    /// continuation line (output of a command, which carries no prefix).
    pub at_ms: Option<u64>,
    pub text: String,
}

/// Parses a raw creation-log line. `None` for blank lines and `=====` separators.
pub fn parse_log_line(raw: &str) -> Option<LogLine> {
    let raw = raw.trim_end();
    let (at_ms, text) = match split_log_prefix(raw) {
        Some((at, rest)) => (Some(at), rest),
        None => (None, raw),
    };
    let text = text.trim();
    if text.is_empty() || text.chars().all(|c| c == '=') {
        return None;
    }
    Some(LogLine {
        at_ms,
        text: text.to_string(),
    })
}

fn split_log_prefix(raw: &str) -> Option<(u64, &str)> {
    // "2026-11-07 04:01:54.456Z: " is 23 characters of time, then "Z: ".
    let stamp = raw.get(..23)?;
    let rest = raw.get(23..)?.strip_prefix('Z')?;
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let parsed = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S%.3f").ok()?;
    let ms = u64::try_from(parsed.and_utc().timestamp_millis()).ok()?;
    Some((ms, rest))
}

#[derive(Deserialize)]
struct StateView {
    state: String,
}

pub fn state(runner: &dyn GhRunner, name: &str) -> Result<String, String> {
    valid_codespace_name(name)?;
    let view: StateView = run_json(
        runner,
        argv(&["codespace", "view", "-c", name, "--json", "state"]),
    )?;
    Ok(view.state)
}

/// Whether a Codespace is in the authenticated user's complete inventory. Read-only;
/// never starts or connects to it. A failed lookup (including 404, which can hide a
/// permission failure) is not evidence that a Codespace was deleted.
pub fn exists(runner: &dyn GhRunner, name: &str) -> Result<bool, String> {
    valid_codespace_name(name)?;
    let args = argv(&["auth", "status", "--active"]);
    let auth = runner.run(&args, None)?;
    if !auth.success {
        return Err(failure(&args, &auth));
    }
    parse_auth_status(&format!("{}\n{}", auth.stdout, auth.stderr)).require_codespace_scope()?;
    #[derive(Deserialize)]
    struct Entry {
        name: String,
    }
    #[derive(Deserialize)]
    struct Page {
        total_count: usize,
        codespaces: Vec<Entry>,
    }
    let pages: Vec<Page> = run_json(
        runner,
        argv(&["api", "--paginate", "--slurp", "user/codespaces"]),
    )?;
    let count: usize = pages.iter().map(|page| page.codespaces.len()).sum();
    let names: HashSet<&str> = pages
        .iter()
        .flat_map(|page| &page.codespaces)
        .map(|entry| entry.name.as_str())
        .collect();
    if pages.is_empty()
        || names.len() != count
        || pages.iter().any(|page| page.total_count != count)
    {
        return Err("Could not read a complete Codespace inventory; try again.".into());
    }
    Ok(pages
        .iter()
        .flat_map(|page| &page.codespaces)
        .any(|entry| entry.name == name))
}

/// Poll until the codespace is `Available`, sleeping through `sleep`.
pub fn wait_ready_with(
    runner: &dyn GhRunner,
    name: &str,
    timeout: Duration,
    interval: Duration,
    sleep: &mut dyn FnMut(Duration),
) -> Result<(), String> {
    let mut waited = Duration::ZERO;
    loop {
        let current = state(runner, name)?;
        match current.as_str() {
            "Available" => return Ok(()),
            "Failed" | "Deleted" | "Unavailable" => {
                return Err(format!("Codespace {name} is {current}"));
            }
            // `gh codespace view` never starts a codespace, so waiting is futile.
            "Shutdown" | "ShuttingDown" => {
                return Err(format!(
                    "Codespace {name} is {current}; start it first (gh codespace ssh starts it)"
                ));
            }
            _ => {}
        }
        if waited >= timeout {
            return Err(format!(
                "Timed out waiting for codespace {name} (last state: {current})"
            ));
        }
        sleep(interval);
        waited += interval;
    }
}

pub fn wait_ready(runner: &dyn GhRunner, name: &str) -> Result<(), String> {
    wait_ready_with(runner, name, READY_TIMEOUT, READY_POLL, &mut |d| {
        std::thread::sleep(d)
    })
}

pub fn stop(runner: &dyn GhRunner, name: &str) -> Result<(), String> {
    valid_codespace_name(name)?;
    run_ok(runner, argv(&["codespace", "stop", "-c", name])).map(|_| ())
}

pub fn delete(runner: &dyn GhRunner, name: &str) -> Result<(), String> {
    valid_codespace_name(name)?;
    run_ok(
        runner,
        argv(&["codespace", "delete", "-c", name, "--force"]),
    )
    .map(|_| ())
}

/// Shell script (for `sh -s`) that prepares the evidence folder of `@WS@`. Idempotent.
///
/// * creates `@WS@/.evidence`;
/// * keeps `.evidence/` and `CLAUDE.local.md` out of git through the repo's `info/exclude`
///   (never `.gitignore`; `git rev-parse --git-path` also finds it for worktree checkouts);
/// * writes the guidance for Claude Code into `@WS@/CLAUDE.local.md` between marker lines.
///   It only writes when the file is absent or a readable, writable regular file that is not
///   a symlink and not tracked by git, and its markers are balanced (none, or exactly one
///   begin before one end). The new content is built in a temp file next to it and moved
///   over it atomically, so a failure at any step leaves the user's file untouched.
///
/// `set -eu` plus the explicit refusals make the exit status reflect "guide not written";
/// callers treat that as best effort.
const EVIDENCE_SCRIPT: &str = r#"set -eu
d='@WS@'
tmp=""
trap 'if [ -n "$tmp" ]; then rm -f "$tmp"; fi' EXIT
mkdir -p "$d/.evidence"
ex=$(git -C "$d" rev-parse --git-path info/exclude 2>/dev/null || true)
case "$ex" in /*) ;; ?*) ex="$d/$ex" ;; esac
if [ -n "$ex" ]; then
  mkdir -p "$(dirname "$ex")"
  if [ -s "$ex" ] && [ -n "$(tail -c1 "$ex")" ]; then echo >> "$ex"; fi
  for p in '.evidence/' 'CLAUDE.local.md'; do
    grep -qxF "$p" "$ex" 2>/dev/null || echo "$p" >> "$ex"
  done
fi
g="$d/CLAUDE.local.md"
skip() { echo "CLAUDE.local.md left alone: $1" >&2; exit 1; }
if [ -L "$g" ]; then skip "it is a symlink"; fi
if [ -e "$g" ]; then
  if [ ! -f "$g" ]; then skip "it is not a regular file"; fi
  if [ ! -r "$g" ] || [ ! -w "$g" ]; then skip "it is not readable and writable"; fi
fi
if git -C "$d" ls-files --error-unmatch CLAUDE.local.md >/dev/null 2>&1; then skip "git tracks it"; fi
begin=0; end=0
if [ -f "$g" ]; then
  begin=$(grep -c '<!-- monoptah-evidence:begin -->' "$g" || true)
  end=$(grep -c '<!-- monoptah-evidence:end -->' "$g" || true)
fi
if [ "$begin" != "$end" ] || [ "$begin" -gt 1 ]; then skip "its Monoptah markers are unbalanced"; fi
if [ "$begin" = 1 ]; then
  bl=$(grep -n '<!-- monoptah-evidence:begin -->' "$g" | cut -d: -f1)
  el=$(grep -n '<!-- monoptah-evidence:end -->' "$g" | cut -d: -f1)
  if [ "$bl" -ge "$el" ]; then skip "its Monoptah markers are out of order"; fi
fi
tmp=$(mktemp "$d/.CLAUDE.local.md.XXXXXX")
if [ -f "$g" ]; then
  if [ "$begin" = 1 ]; then
    awk '/<!-- monoptah-evidence:begin -->/{s=1} !s{print} /<!-- monoptah-evidence:end -->/{s=0}' "$g" > "$tmp"
  else
    cat "$g" > "$tmp"
    if [ -s "$tmp" ] && [ -n "$(tail -c1 "$tmp")" ]; then echo >> "$tmp"; fi
  fi
  chmod "$(stat -c %a "$g" 2>/dev/null || echo 644)" "$tmp"
else
  chmod 644 "$tmp"
fi
cat >> "$tmp" <<'GUIDE'
<!-- monoptah-evidence:begin -->
## Evidence for the local user
You run in @PLACE@. Save screenshots, test output and written reports that prove
your work under `@WS@/.evidence/` (create subfolders freely). That folder is excluded
from git and is never committed. The user's Monoptah app copies it to their machine on request,
so do not put secrets there. Mention the file names in your final message.
The user can click a file path in the chat to view it.
<!-- monoptah-evidence:end -->
GUIDE
mv "$tmp" "$g"
tmp=""
"#;

/// The evidence script for `workspace`, with the guide naming the place the agent runs in.
/// The begin/end markers do not depend on `kind`, so a file written by one is updated in place
/// by the other.
fn evidence_script(workspace: &str, kind: CloudBackendKind) -> String {
    let place = match kind {
        CloudBackendKind::Codespaces => "a GitHub codespace",
        CloudBackendKind::DockerSandbox => "a Docker sandbox",
    };
    EVIDENCE_SCRIPT
        .replace("@PLACE@", place)
        .replace("@WS@", workspace)
}

/// The evidence setup script for `workspace` (`/workspaces/<repo>`), validated, for backends
/// that run it themselves.
pub(crate) fn evidence_script_for(
    workspace: &str,
    kind: CloudBackendKind,
) -> Result<String, String> {
    let repo = workspace
        .strip_prefix("/workspaces/")
        .filter(|repo| valid_segment(repo))
        .ok_or_else(|| format!("Invalid workspace `{workspace}`"))?;
    Ok(evidence_script(&format!("/workspaces/{repo}"), kind))
}

/// The evidence setup script for the absolute `workspace` of a session that does not live under
/// `/workspaces` (a sandbox without a devcontainer).
pub(crate) fn evidence_script_at(
    workspace: &str,
    kind: CloudBackendKind,
) -> Result<String, String> {
    valid_remote_path(workspace)?;
    if !workspace.starts_with('/') || workspace.len() < 2 {
        return Err(format!("Invalid workspace `{workspace}`"));
    }
    Ok(evidence_script(workspace.trim_end_matches('/'), kind))
}

/// Best-effort setup of the evidence folder and agent guidance in a running codespace.
/// `workspace` is the repo checkout, `/workspaces/<repo>`. Bounded by `SSH_TIMEOUT` (30s).
pub fn prepare_evidence(runner: &dyn GhRunner, name: &str, workspace: &str) -> Result<(), String> {
    valid_codespace_name(name)?;
    let repo = workspace
        .strip_prefix("/workspaces/")
        .filter(|repo| valid_segment(repo))
        .ok_or_else(|| format!("Invalid workspace `{workspace}`"))?;
    let args = argv(&["codespace", "ssh", "-c", name, "--", "sh", "-s"]);
    let script = evidence_script(&format!("/workspaces/{repo}"), CloudBackendKind::Codespaces);
    let out = runner.run(&args, Some(&script))?;
    if out.success {
        Ok(())
    } else {
        Err(failure(&args, &out))
    }
}

/// Runs `script` with `sh -s` inside the codespace (it starts a stopped one) and returns
/// stdout. `long` allows minutes instead of 30 seconds.
pub fn run_script(
    runner: &dyn GhRunner,
    name: &str,
    script: &str,
    long: bool,
) -> Result<String, String> {
    valid_codespace_name(name)?;
    let mut parts = vec!["codespace", "ssh", "-c", name, "--", "sh", "-s"];
    if long {
        parts.push("--");
        parts.push(LONG_SCRIPT_MARKER);
    }
    let args = argv(&parts);
    let out = runner.run(&args, Some(script))?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(failure(&args, &out))
    }
}

/// Checks a path inside a codespace before it is handed to `gh codespace cp`.
///
/// gh without `-e` quotes the remote name in a way modern `scp` (SFTP mode) takes literally, so
/// every copy fails with "No such file or directory" (verified against a real codespace). We
/// therefore pass `-e`, which hands the name to the remote shell: only a conservative character
/// set is accepted, so nothing can expand, split or look like an option, and `..` is refused.
pub fn valid_remote_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 1024 {
        return Err("Remote path must be 1 to 1024 characters".into());
    }
    if path.starts_with('-') {
        return Err("Remote path must not start with `-`".into());
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-' | b'+' | b'@'))
    {
        return Err(
            "Remote path may only use letters, digits and . _ / - + @ (no spaces or shell characters)"
                .into(),
        );
    }
    if path.split('/').any(|part| part == ".." || part == ".") {
        return Err("Remote path must not contain `.` or `..` segments".into());
    }
    Ok(())
}

/// True when a `copy_from` error says `remote_path` ITSELF does not exist. A nested failure
/// (a dangling symlink or a file that vanished inside the folder) names a longer path and is
/// not "missing": scp still copied the rest.
pub fn is_missing_path(error: &str, remote_path: &str) -> bool {
    error.lines().any(|line| {
        [format!("{remote_path}': "), format!("{remote_path}: ")]
            .iter()
            .any(|key| {
                line.split_once(key.as_str())
                    .is_some_and(|(_, rest)| rest.starts_with("No such file or directory"))
            })
    })
}

/// Copies `remote_path` (a file or directory inside the codespace, see [`valid_remote_path`]) into the existing local
/// directory `local_dir`, recursively, like `cp -r remote_path local_dir`: a directory arrives
/// as `local_dir/<its name>`. Starts a stopped codespace (gh does). A missing remote path is an
/// error for which [`is_missing_path`] is true. Needs an absolute `local_dir`.
pub fn copy_from(
    runner: &dyn GhRunner,
    name: &str,
    remote_path: &str,
    local_dir: &Path,
) -> Result<(), String> {
    valid_codespace_name(name)?;
    valid_remote_path(remote_path)?;
    // scp reads `host:path` for a relative local path with a colon; absolute paths are safe.
    if !local_dir.is_absolute() {
        return Err("Local destination must be an absolute path".into());
    }
    let local = local_dir
        .to_str()
        .ok_or("Local destination is not valid UTF-8")?;
    run_ok(
        runner,
        vec![
            "codespace".into(),
            "cp".into(),
            "-e".into(),
            "-r".into(),
            "-c".into(),
            name.into(),
            format!("remote:{remote_path}"),
            local.into(),
        ],
    )
    .map(|_| ())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MachineType {
    pub name: String,
    pub display_name: String,
    pub operating_system: String,
    pub storage_in_bytes: u64,
    pub memory_in_bytes: u64,
    pub cpus: u32,
}

/// The machine a new codespace starts on when the user does not choose: `preferred` (the
/// project's machine) while the repository offers it, else the smallest machine it offers
/// ([`pick_machine`]). `None` when the list is empty.
pub fn default_machine(machines: &[MachineType], preferred: Option<&str>) -> Option<String> {
    (!machines.is_empty()).then(|| pick_machine(machines, preferred).name)
}

/// The REST API uses snake_case; accept both so tests and gh versions agree.
fn parse_machines(text: &str) -> Result<Vec<MachineType>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Unexpected gh output: {e}"))?;
    let machines = value
        .get("machines")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(machines
        .iter()
        .filter_map(|m| {
            let text = |a: &str, b: &str| {
                m.get(a)
                    .or_else(|| m.get(b))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let num = |a: &str, b: &str| {
                m.get(a)
                    .or_else(|| m.get(b))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            };
            let name = text("name", "name");
            (!name.is_empty()).then(|| MachineType {
                display_name: text("display_name", "displayName"),
                operating_system: text("operating_system", "operatingSystem"),
                storage_in_bytes: num("storage_in_bytes", "storageInBytes"),
                memory_in_bytes: num("memory_in_bytes", "memoryInBytes"),
                cpus: num("cpus", "cpus") as u32,
                name,
            })
        })
        .collect())
}

pub fn list_machines(runner: &dyn GhRunner, repo: &str) -> Result<Vec<MachineType>, String> {
    list_machines_at(runner, repo, None)
}

/// The machine types GitHub lets this repository create a codespace with. With `git_ref` the
/// answer honours that branch's devcontainer (`hostRequirements` hides machines that are too
/// small), so `basicLinux32gb` is missing for a repository that needs 4 cores.
pub fn list_machines_at(
    runner: &dyn GhRunner,
    repo: &str,
    git_ref: Option<&str>,
) -> Result<Vec<MachineType>, String> {
    valid_repo(repo)?;
    let mut path = format!("/repos/{repo}/codespaces/machines");
    if let Some(git_ref) = git_ref.map(str::trim).filter(|r| !r.is_empty()) {
        let encoded: String = url::form_urlencoded::byte_serialize(git_ref.as_bytes()).collect();
        path.push_str(&format!("?ref={encoded}"));
    }
    parse_machines(&run_ok(runner, argv(&["api", &path]))?)
}

/// The cheapest machine: fewest cores, then least memory, then least storage (name breaks a
/// tie so the answer never depends on GitHub's order).
pub fn smallest_machine(machines: &[MachineType]) -> Option<&MachineType> {
    machines.iter().min_by(|a, b| {
        (a.cpus, a.memory_in_bytes, a.storage_in_bytes, &a.name).cmp(&(
            b.cpus,
            b.memory_in_bytes,
            b.storage_in_bytes,
            &b.name,
        ))
    })
}

/// The machine a new codespace gets, from what the repository allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachinePick {
    pub name: String,
    /// The preferred machine that is no longer allowed and was replaced by `name`.
    pub replaced: Option<String>,
}

/// `preferred` when the repository still allows it, else the smallest allowed machine. An
/// empty `allowed` means GitHub did not say (offline, no scope, an older gh): `preferred` or
/// [`DEFAULT_MACHINE`] then, as before the lookup existed.
pub fn pick_machine(allowed: &[MachineType], preferred: Option<&str>) -> MachinePick {
    let preferred = preferred.map(str::trim).filter(|m| !m.is_empty());
    if allowed.is_empty() {
        return MachinePick {
            name: preferred.unwrap_or(DEFAULT_MACHINE).to_owned(),
            replaced: None,
        };
    }
    if let Some(name) = preferred.filter(|name| allowed.iter().any(|m| m.name == *name)) {
        return MachinePick {
            name: name.to_owned(),
            replaced: None,
        };
    }
    MachinePick {
        name: smallest_machine(allowed)
            .map_or_else(|| DEFAULT_MACHINE.to_owned(), |m| m.name.clone()),
        replaced: preferred.map(str::to_owned),
    }
}

/// Allowed machine types by `(repo, ref)`, kept for the life of the process: they change only
/// when a devcontainer's `hostRequirements` or the account's policy does. Only an answer that
/// names machines is kept, so a failed lookup (offline, auth) is asked again next time.
#[derive(Default)]
pub struct MachineCache {
    entries: std::sync::Mutex<std::collections::HashMap<(String, String), Vec<MachineType>>>,
}

impl MachineCache {
    /// The allowed machines of `repo` at `git_ref`, from the cache or one `gh api` call.
    pub fn allowed(
        &self,
        runner: &dyn GhRunner,
        repo: &str,
        git_ref: Option<&str>,
    ) -> Result<Vec<MachineType>, String> {
        let key = (
            repo.to_owned(),
            git_ref.map(str::trim).unwrap_or_default().to_owned(),
        );
        if let Some(hit) = self.entries.lock().ok().and_then(|e| e.get(&key).cloned()) {
            return Ok(hit);
        }
        let machines = list_machines_at(runner, repo, git_ref)?;
        if machines.is_empty() {
            return Err(format!("GitHub listed no machine types for {repo}"));
        }
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(key, machines.clone());
        }
        Ok(machines)
    }

    /// The machine for a new codespace of `repo` on `git_ref` when the caller names none (or
    /// names one the repository no longer allows): the smallest allowed one. A failed lookup
    /// keeps `preferred` or [`DEFAULT_MACHINE`], so it never blocks a start.
    pub fn pick(
        &self,
        runner: &dyn GhRunner,
        repo: &str,
        git_ref: Option<&str>,
        preferred: Option<&str>,
    ) -> MachinePick {
        let allowed = self.allowed(runner, repo, git_ref).unwrap_or_default();
        pick_machine(&allowed, preferred)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevcontainerSecret {
    pub key: String,
    pub description: String,
}

/// Remove `//` and `/* */` comments and trailing commas, leaving strings alone.
pub fn strip_jsonc(input: &str) -> String {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if chars[i - 1] == '"' {
                    break;
                }
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            out.push(' ');
        } else if c == ',' {
            let mut j = i + 1;
            while j < chars.len() {
                if chars[j].is_whitespace() {
                    j += 1;
                } else if chars[j] == '/' && chars.get(j + 1) == Some(&'/') {
                    while j < chars.len() && chars[j] != '\n' {
                        j += 1;
                    }
                } else if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
                    j += 2;
                    while j < chars.len() && !(chars[j] == '*' && chars.get(j + 1) == Some(&'/')) {
                        j += 1;
                    }
                    j += 2;
                } else {
                    break;
                }
            }
            if !matches!(chars.get(j), Some('}') | Some(']')) {
                out.push(c);
            }
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn devcontainer_files(project_dir: &Path) -> Vec<std::path::PathBuf> {
    let dir = project_dir.join(".devcontainer");
    let mut files = vec![dir.join("devcontainer.json")];
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut nested: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .map(|p| p.join("devcontainer.json"))
            .collect();
        nested.sort();
        files.extend(nested);
    }
    files.push(project_dir.join(".devcontainer.json"));
    files.into_iter().filter(|f| f.is_file()).collect()
}

/// Secrets declared in the project's devcontainer configs, deduped by key.
pub fn devcontainer_secrets(project_dir: &Path) -> Vec<DevcontainerSecret> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for file in devcontainer_files(project_dir) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&text)) else {
            continue;
        };
        let Some(secrets) = value.get("secrets").and_then(|s| s.as_object()) else {
            continue;
        };
        for (key, meta) in secrets {
            if seen.insert(key.clone()) {
                found.push(DevcontainerSecret {
                    key: key.clone(),
                    description: meta
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }
    }
    found
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretState {
    Set,
    Missing,
    NotForRepo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub key: String,
    pub status: SecretState,
}

pub fn valid_secret_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn secret_repos(runner: &dyn GhRunner, key: &str) -> Result<Vec<String>, String> {
    let path = format!("/user/codespaces/secrets/{key}/repositories?per_page=100");
    Ok(collect_field(
        &api_pages(runner, &path)?,
        "repositories",
        "full_name",
    ))
}

/// Status of each key (plus `CLAUDE_CODE_OAUTH_TOKEN`) for this repo. Values
/// are never read: GitHub does not return them.
pub fn secrets_status(
    runner: &dyn GhRunner,
    repo: &str,
    keys: &[String],
) -> Result<Vec<SecretStatus>, String> {
    valid_repo(repo)?;
    let mut wanted = vec![OAUTH_SECRET_KEY.to_string()];
    for key in keys {
        if !wanted.contains(key) {
            wanted.push(key.clone());
        }
    }
    if let Some(bad) = wanted.iter().find(|k| !valid_secret_key(k)) {
        return Err(format!("Invalid secret name `{bad}`"));
    }
    let existing: HashSet<String> = collect_field(
        &api_pages(runner, "/user/codespaces/secrets?per_page=100")?,
        "secrets",
        "name",
    )
    .into_iter()
    .collect();
    let mut result = Vec::new();
    for key in wanted {
        let status = if !existing.contains(&key) {
            SecretState::Missing
        } else {
            let selected = secret_repos(runner, &key)?
                .iter()
                .any(|name| name.eq_ignore_ascii_case(repo));
            if selected {
                SecretState::Set
            } else {
                SecretState::NotForRepo
            }
        };
        result.push(SecretStatus { key, status });
    }
    Ok(result)
}

/// Clean a pasted secret value. `claude setup-token` wraps its output across lines, so a
/// copied CLAUDE_CODE_OAUTH_TOKEN carries whitespace in the middle: strip all of it and
/// check the result looks like a token. Other secrets only lose leading/trailing
/// whitespace. Errors never contain the value.
pub fn normalize_secret_value(key: &str, value: &str) -> Result<String, String> {
    if key != OAUTH_SECRET_KEY {
        return Ok(value.trim().to_owned());
    }
    let token: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    let well_formed = token.starts_with("sk-ant-")
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !well_formed {
        return Err(format!(
            "{key} does not look like a Claude token: it should start with sk-ant- and contain only letters, digits, - and _. Paste the full output of `claude setup-token`."
        ));
    }
    Ok(token)
}

/// Store a user codespaces secret for `repo`. The value goes over stdin only. This is the
/// single entry point for writing secrets, so the value is normalized here.
pub fn set_secret(runner: &dyn GhRunner, key: &str, value: &str, repo: &str) -> Result<(), String> {
    valid_repo(repo)?;
    if !valid_secret_key(key) {
        return Err(format!("Invalid secret name `{key}`"));
    }
    let value = normalize_secret_value(key, value)?;
    let value = value.as_str();
    if value.is_empty() {
        return Err("Secret value is empty".into());
    }
    // `--repos` replaces the selection, so keep the repos the secret already has.
    let mut repos = match secret_repos(runner, key) {
        Ok(repos) => repos,
        Err(error) if error.contains("404") || error.to_lowercase().contains("not found") => {
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    if !repos.iter().any(|r| r.eq_ignore_ascii_case(repo)) {
        repos.push(repo.to_string());
    }
    let args = argv(&[
        "secret",
        "set",
        key,
        "--app",
        "codespaces",
        "--user",
        "--repos",
        &repos.join(","),
    ]);
    let out = runner.run(&args, Some(value))?;
    if out.success {
        Ok(())
    } else {
        // Never echo gh output verbatim if it somehow contains the value.
        Err(failure(&args, &out).replace(value, "***"))
    }
}

#[cfg(test)]
mod tests;
