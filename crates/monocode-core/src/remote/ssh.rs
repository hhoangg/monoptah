use super::ssh_askpass::Askpass;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, PoisonError,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshTarget {
    pub target: String,
    pub port: Option<u16>,
    pub remote_port: u16,
    /// GitHub Codespace name. When set, every connection goes through
    /// `gh codespace ssh -c NAME --` instead of plain `ssh`, and `target`
    /// only mirrors the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codespace: Option<String>,
    /// Docker Sandbox. When set, every connection goes through plain `ssh` to
    /// `<id>@sbx_cloud` (the alias `sbx setup ssh` writes), and commands that
    /// belong to the Host run inside the devcontainer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxTarget>,
}

/// Where a sandbox session's Host and agent run.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SandboxMode {
    /// In the repository's devcontainer. The default, so a record or target written before
    /// this field existed keeps its meaning.
    #[default]
    Container,
    /// Directly in the outer sandbox: the repository has no devcontainer config.
    Plain,
}

impl SandboxMode {
    pub fn is_container(&self) -> bool {
        *self == Self::Container
    }
}

/// A Docker Sandbox that runs the Host inside the repository's devcontainer, or (when the
/// repository has none, see [`SandboxMode::Plain`]) directly in the outer sandbox.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxTarget {
    /// Sandbox id (`sbx_...`), also the ssh user.
    pub id: String,
    /// Sandbox name, for display and the machine endpoint.
    pub name: String,
    /// Workspace folder in the OUTER sandbox, the devcontainer's workspace.
    pub workspace: String,
    /// Loopback port in the outer sandbox where a relay bridges into the container's Host;
    /// the tunnel forwards to it. Unused in [`SandboxMode::Plain`]: there is no relay and the
    /// tunnel goes to the Host's own port.
    pub relay_port: u16,
    /// Dev container config, relative to `workspace`, when it is not the default one
    /// (`devcontainer exec` finds its container by workspace and config file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Container or plain outer sandbox. Absent in older records: Container.
    #[serde(default, skip_serializing_if = "SandboxMode::is_container")]
    pub mode: SandboxMode,
    /// The Docker account the sandbox lives in (an id from `docker_accounts`). Every ssh to
    /// the sandbox runs with that account's `DOCKER_ACCESS_TOKEN`, because the alias's
    /// ProxyCommand is `sbx --cloud ssh-connect`. Absent: the machine's `sbx login`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_account: Option<String>,
}

/// Host alias written by `sbx setup ssh` into the user's ssh config.
const SANDBOX_SSH_HOST: &str = "sbx_cloud";
/// Where the standalone devcontainer CLI lives in the outer sandbox. It is not
/// on PATH; `$HOME` is expanded by the outer sandbox's shell.
const SANDBOX_DEVCONTAINER: &str = "$HOME/.devcontainers/bin/devcontainer";

impl SandboxTarget {
    pub(super) fn destination(&self) -> String {
        format!("{}@{SANDBOX_SSH_HOST}", self.id)
    }

    /// The single remote word that runs `remote` where the Host lives: inside the
    /// devcontainer, or, in [`SandboxMode::Plain`], in the workspace of the outer sandbox. The
    /// script itself travels on stdin, never in argv.
    pub(crate) fn exec_in_container(&self, remote: &[&str]) -> String {
        if self.mode == SandboxMode::Plain {
            let words: Vec<String> = remote.iter().map(|word| quote_word(word)).collect();
            return format!("cd {} && {}", shell_quote(&self.workspace), words.join(" "));
        }
        let mut words = vec![
            SANDBOX_DEVCONTAINER.to_string(),
            "exec".into(),
            "--workspace-folder".into(),
            shell_quote(&self.workspace),
        ];
        if let Some(config) = &self.config {
            words.push("--config".into());
            words.push(shell_quote(&format!("{}/{config}", self.workspace)));
        }
        words.push("--".into());
        words.extend(remote.iter().map(|word| quote_word(word)));
        words.join(" ")
    }
}

/// How a target is reached. Derived from the optional fields of `SshTarget`.
enum Transport<'a> {
    Ssh,
    Codespace(&'a str),
    Sandbox(&'a SandboxTarget),
}

impl SshTarget {
    fn transport(&self) -> Transport<'_> {
        match (&self.codespace, &self.sandbox) {
            (Some(name), _) => Transport::Codespace(name),
            (None, Some(sandbox)) => Transport::Sandbox(sandbox),
            (None, None) => Transport::Ssh,
        }
    }

    /// True for targets whose connection wakes a paid cloud resource, so
    /// background work must never open a connection to them.
    pub fn is_cloud(&self) -> bool {
        !matches!(self.transport(), Transport::Ssh)
    }
}

/// Quote a word only when the shell would treat it specially.
fn quote_word(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@%+,".contains(&b))
    {
        word.into()
    } else {
        shell_quote(word)
    }
}

/// Sandbox ids are `sbx_` plus lowercase alphanumerics.
pub fn validate_sandbox_id(id: &str) -> Result<(), String> {
    match id.strip_prefix("sbx_") {
        Some(rest)
            if !rest.is_empty()
                && id.len() <= 64
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => {
            Err("Enter a valid sandbox id (sbx_ followed by lowercase letters and digits).".into())
        }
    }
}

/// Sandbox names follow the `sbx` rules; the one implementation lives in [`crate::sandboxes`].
pub use crate::sandboxes::validate_sandbox_name;

/// The workspace path reaches a remote shell (quoted). Require an absolute
/// path of plain characters with no `..` or empty components.
pub fn validate_sandbox_workspace(path: &str) -> Result<(), String> {
    let valid = path.len() > 1
        && path.len() <= 255
        && path.starts_with('/')
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
        && path[1..]
            .trim_end_matches('/')
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if valid {
        Ok(())
    } else {
        Err("Enter an absolute workspace path (letters, digits, / . _ -).".into())
    }
}

/// Re-checks everything of a sandbox target that reaches argv.
pub fn validate_sandbox(sandbox: &SandboxTarget) -> Result<(), String> {
    validate_sandbox_id(&sandbox.id)?;
    validate_sandbox_name(&sandbox.name)?;
    validate_sandbox_workspace(&sandbox.workspace)?;
    if sandbox.relay_port == 0 {
        return Err("Enter a valid relay port.".into());
    }
    if let Some(config) = &sandbox.config {
        validate_sandbox_workspace(&format!("/{config}"))
            .map_err(|_| "Enter a relative dev container config path.".to_string())?;
    }
    Ok(())
}

/// Codespace names are generated by GitHub (`user-repo-abc123`). They end up in
/// argv, so reject anything that could be read as an option or shell syntax.
pub fn validate_codespace(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 100
        || name.starts_with('-')
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Enter a valid codespace name (letters, digits and hyphens).".into());
    }
    Ok(name.into())
}

pub fn validate_target(target: &str, port: Option<u16>) -> Result<String, String> {
    let target = target.trim();
    if target.is_empty()
        || target.len() > 255
        || target.starts_with('-')
        || port == Some(0)
        || !target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-@:[ ]".contains(&b) && b != b' ')
        || target.matches('@').count() > 1
        || target.starts_with('@')
        || target.ends_with('@')
    {
        return Err(
            "Enter an SSH hostname or alias, such as user@my-mac-mini, and a valid port.".into(),
        );
    }
    Ok(target.into())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    pub id: String,
    pub message: String,
    pub confirm: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    pub message: String,
    pub prompt: Option<Prompt>,
    pub done: bool,
    pub error: Option<String>,
    pub machine: Option<crate::remote::Machine>,
}
struct JobData {
    view: JobView,
    answer: Option<String>,
    /// When the job completed; finished jobs are kept a while for their owner to read.
    finished: Option<Instant>,
}

/// What a setup job connects to. Two jobs for different cloud targets can run side by side;
/// jobs for the same target, or any job on a plain SSH machine, cannot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobTarget {
    Codespace(String),
    Sandbox(String),
    /// Destination and port of a plain SSH machine.
    Ssh(String, Option<u16>),
}

impl SshTarget {
    pub fn job_target(&self) -> JobTarget {
        match self.transport() {
            Transport::Codespace(name) => JobTarget::Codespace(name.into()),
            Transport::Sandbox(sandbox) => JobTarget::Sandbox(sandbox.name.clone()),
            Transport::Ssh => JobTarget::Ssh(self.target.clone(), self.port),
        }
    }
}

/// True when an active job for `active` must finish before a job for `new` may start.
/// `None` (a job tied to no single target) conflicts with everything.
///
/// * The same target twice would race on one machine.
/// * A plain SSH job can ask for a password or a host key and writes `~/.ssh/known_hosts`,
///   so it runs alone.
/// * Codespace jobs for different codespaces run together, but only once `gh` has created
///   its automatic key pair (`key_ready`): the first `gh codespace ssh` generates it, and
///   two first runs could race on the files.
pub fn jobs_conflict(new: Option<&JobTarget>, active: Option<&JobTarget>, key_ready: bool) -> bool {
    let (Some(new), Some(active)) = (new, active) else {
        return true;
    };
    match (new, active) {
        (JobTarget::Ssh(..), _) | (_, JobTarget::Ssh(..)) => true,
        (new, active) if new == active => true,
        (JobTarget::Codespace(_), JobTarget::Codespace(_)) => !key_ready,
        _ => false,
    }
}

/// Whether `gh` has already created its automatic key pair for `gh codespace ssh`
/// (`~/.ssh/codespaces.auto` and `.pub`). Without a known home it counts as missing, which
/// only makes codespace setups run one at a time.
pub fn codespace_key_exists() -> bool {
    #[cfg(all(test, unix))]
    let override_home = HOME_OVERRIDE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    #[cfg(not(all(test, unix)))]
    let override_home: Option<std::path::PathBuf> = None;
    let Some(home) = override_home.or_else(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
    }) else {
        return false;
    };
    let ssh = home.join(".ssh");
    ssh.join("codespaces.auto").is_file() && ssh.join("codespaces.auto.pub").is_file()
}

pub struct Job {
    inner: Mutex<JobData>,
    pub cancelled: AtomicBool,
    /// `None` for a job that is not tied to one target (a Host upgrade slot): it conflicts
    /// with every other job.
    target: Option<JobTarget>,
}
impl Job {
    pub fn new() -> Arc<Self> {
        Self::with_target(None)
    }
    pub fn for_target(target: JobTarget) -> Arc<Self> {
        Self::with_target(Some(target))
    }
    fn with_target(target: Option<JobTarget>) -> Arc<Self> {
        Arc::new(Self {
            target,
            inner: Mutex::new(JobData {
                view: JobView {
                    id: uuid::Uuid::new_v4().to_string(),
                    message: "Connecting to SSH and setting up Monoptah Host…".into(),
                    prompt: None,
                    done: false,
                    error: None,
                    machine: None,
                },
                answer: None,
                finished: None,
            }),
            cancelled: AtomicBool::new(false),
        })
    }
    pub fn target(&self) -> Option<&JobTarget> {
        self.target.as_ref()
    }
    /// When the job completed, `None` while it runs.
    pub fn finished_at(&self) -> Option<Instant> {
        self.inner.lock().unwrap().finished
    }
    pub fn view(&self) -> JobView {
        self.inner.lock().unwrap().view.clone()
    }
    pub fn message(&self, text: &str) {
        self.inner.lock().unwrap().view.message = text.into();
    }
    pub fn complete(&self, action: impl FnOnce() -> Result<crate::remote::Machine, String>) {
        let mut inner = self.inner.lock().unwrap();
        let result = if self.cancelled.load(Ordering::Relaxed) {
            Err("Connection cancelled".into())
        } else {
            action()
        };
        inner.view.done = true;
        inner.finished = Some(Instant::now());
        inner.view.prompt = None;
        inner.answer = None;
        match result {
            Ok(machine) => {
                inner.view.message = "Connected".into();
                inner.view.machine = Some(machine);
            }
            Err(error) => inner.view.error = Some(error),
        }
    }
    pub fn cancel(&self) {
        let inner = self.inner.lock().unwrap();
        if !inner.view.done {
            self.cancelled.store(true, Ordering::Relaxed);
        }
    }
    pub fn answer(&self, id: &str, answer: String) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "SSH prompt is unavailable")?;
        let prompt = inner
            .view
            .prompt
            .as_ref()
            .filter(|p| p.id == id)
            .ok_or("This SSH prompt has expired")?;
        if answer.len() > 8192
            || answer.contains(['\n', '\r', '\0'])
            || (prompt.confirm && answer != "yes" && answer != "no")
        {
            return Err("Invalid SSH prompt response".into());
        }
        inner.answer = Some(answer);
        Ok(())
    }
    fn prompt(&self, message: String, confirm: bool) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(120);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.answer = None;
            inner.view.prompt = Some(Prompt {
                id: uuid::Uuid::new_v4().to_string(),
                message,
                confirm,
            });
        }
        loop {
            let mut inner = self.inner.lock().unwrap();
            if self.cancelled.load(Ordering::Relaxed)
                || Instant::now() >= deadline
                || inner.view.done
            {
                inner.view.prompt = None;
                inner.answer = None;
                return None;
            }
            if let Some(answer) = inner.answer.take() {
                inner.view.prompt = None;
                return Some(answer);
            }
            drop(inner);
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn askpass(self: &Arc<Self>) -> Result<Askpass, String> {
        let job = self.clone();
        Askpass::start(move |message, confirm| job.prompt(message, confirm))
    }
}

#[cfg(all(test, unix))]
static GH_OVERRIDE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
#[cfg(all(test, unix))]
static GH_LOCK: Mutex<()> = Mutex::new(());
#[cfg(all(test, unix))]
static HOME_OVERRIDE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// Points codespace commands at a fake `gh` for the life of the guard. The
/// guard also serializes tests, because the override is process-global.
#[cfg(all(test, unix))]
pub(super) struct GhOverride(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
#[cfg(all(test, unix))]
impl GhOverride {
    pub(super) fn set(path: std::path::PathBuf) -> Self {
        let guard = GH_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        *GH_OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner) = Some(path);
        Self(guard)
    }
}
#[cfg(all(test, unix))]
impl GhOverride {
    /// Makes the codespace key lookup read this directory instead of the real home.
    pub(super) fn set_home(&self, home: std::path::PathBuf) {
        *HOME_OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner) = Some(home);
    }
}
#[cfg(all(test, unix))]
impl Drop for GhOverride {
    fn drop(&mut self) {
        *GH_OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner) = None;
        *HOME_OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

fn gh_program() -> std::ffi::OsString {
    #[cfg(all(test, unix))]
    if let Some(path) = GH_OVERRIDE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return path.into();
    }
    crate::scm::resolve_gui_binary("gh")
        .map(Into::into)
        .unwrap_or_else(|| "gh".into())
}

fn command(target: &SshTarget, interactive: bool) -> Result<Command, String> {
    let program = match target.transport() {
        Transport::Codespace(_) => gh_program(),
        Transport::Ssh | Transport::Sandbox(_) => "ssh".into(),
    };
    command_with_program(target, interactive, &program)
}

/// `gh codespace ssh` appends its own destination and hands everything after
/// `--` to OpenSSH, so codespace commands carry no `-- TARGET` and no `-p`.
/// gh also manages the host key and key pair itself, so host-key prompting is
/// left out and BatchMode stays on. A sandbox is plain `ssh` to its
/// `<id>@sbx_cloud` alias: the alias carries the ProxyCommand and host trust,
/// so there is no host-key prompting, no password prompt and no `-p`.
///
/// A sandbox that lives in a Docker account gets that account's `DOCKER_ACCESS_TOKEN` in
/// the environment (the ProxyCommand's `sbx` reads it; never argv). An account that cannot
/// produce a token is an error, not a silent fallback to the machine login. A long-lived
/// tunnel keeps the environment it was spawned with; a reconnect spawns a new ssh and gets
/// a fresh token.
fn command_with_program(
    target: &SshTarget,
    interactive: bool,
    program: &std::ffi::OsStr,
) -> Result<Command, String> {
    let mut command = Command::new(program);
    let transport = target.transport();
    if let Transport::Codespace(name) = transport {
        command.args(["codespace", "ssh", "-c", name, "--"]);
    }
    let plain = matches!(transport, Transport::Ssh);
    // Resuming a stopped sandbox through the sbx ProxyCommand can outlast the
    // usual 15 seconds before the ssh banner arrives.
    let connect_timeout = match transport {
        Transport::Sandbox(_) => "ConnectTimeout=120",
        Transport::Ssh | Transport::Codespace(_) => "ConnectTimeout=15",
    };
    command.args([
        "-T",
        "-o",
        connect_timeout,
        "-o",
        "ConnectionAttempts=1",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "ExitOnForwardFailure=yes",
    ]);
    if plain {
        command.args([
            "-o",
            "StrictHostKeyChecking=ask",
            "-o",
            "NumberOfPasswordPrompts=3",
        ]);
    }
    command.args(["-o", "ForkAfterAuthentication=no"]);
    command.args([
        "-o",
        if interactive && plain {
            "BatchMode=no"
        } else {
            "BatchMode=yes"
        },
    ]);
    if let Some(port) = target.port.filter(|_| plain) {
        command.args(["-p", &port.to_string()]);
    }
    command
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !plain {
        // A Finder-launched app has launchd's bare PATH, and gh runs `ssh` and
        // the sandbox alias's ProxyCommand runs `sbx`.
        command.env("PATH", crate::scm::terminal_search_path());
    }
    if let Transport::Sandbox(sandbox) = transport {
        crate::docker_accounts::apply_env(&mut command, sandbox.docker_account.as_deref())?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    Ok(command)
}

/// Names and paths read back from the store are re-checked before they reach argv.
pub(super) fn check_target(target: &SshTarget) -> Result<(), String> {
    if target.codespace.is_some() && target.sandbox.is_some() {
        return Err("A connection cannot be both a codespace and a sandbox.".into());
    }
    match target.transport() {
        Transport::Codespace(name) => validate_codespace(name).map(|_| ()),
        Transport::Sandbox(sandbox) => validate_sandbox(sandbox),
        Transport::Ssh => Ok(()),
    }
}

/// Error for a transport process that could not be spawned.
fn spawn_error(target: &SshTarget, error: std::io::Error) -> String {
    match target.transport() {
        Transport::Ssh | Transport::Sandbox(_) => format!("Could not start OpenSSH: {error}"),
        Transport::Codespace(_) if error.kind() == std::io::ErrorKind::NotFound => {
            "GitHub CLI (gh) is not installed. Install it from https://cli.github.com and run `gh auth login`.".into()
        }
        Transport::Codespace(_) => format!("Could not start the GitHub CLI (gh): {error}"),
    }
}

fn capture(mut reader: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let remaining = (64 * 1024usize).saturating_sub(output.len());
            output.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        output
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPlatform {
    Unix,
    Windows,
}

const PLATFORM_PROBE: &[&str] = &["echo", "MONOCODE_PLATFORM", "$env:OS", "%OS%", "$OS"];

fn parse_platform(output: &str) -> Result<HostPlatform, String> {
    let marker = output
        .rsplit_once("MONOCODE_PLATFORM")
        .ok_or("Could not identify the remote shell. Use cmd.exe, PowerShell, or a Unix shell.")?
        .1;
    Ok(
        if marker
            .split_whitespace()
            .any(|word| word.eq_ignore_ascii_case("Windows_NT"))
        {
            HostPlatform::Windows
        } else {
            HostPlatform::Unix
        },
    )
}

pub fn detect_platform(
    target: &SshTarget,
    job: &Arc<Job>,
    askpass: &Askpass,
) -> Result<HostPlatform, String> {
    if target.is_cloud() {
        // Codespaces and sandboxes are always Linux containers.
        return Ok(HostPlatform::Unix);
    }
    job.message("Checking the remote machine…");
    let output = run_remote_command(
        target,
        String::new(),
        job,
        askpass,
        command(target, true)?,
        PLATFORM_PROBE,
    )?;
    parse_platform(&output)
}

fn powershell_encoded(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Keep the remote command below cmd.exe's length limit. The actual script is
/// read from UTF-8 stdin as a single block, rather than evaluated line by line.
fn powershell_reader() -> String {
    // Prefer this shell's built-in modules if the SSH environment inherited
    // PowerShell 7 module paths through an intermediate process.
    powershell_encoded("$env:PSModulePath = $PSHOME + '\\Modules;' + $env:PSModulePath; $ErrorActionPreference = 'Stop'; [Console]::InputEncoding = [Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); try { & ([ScriptBlock]::Create([Console]::In.ReadToEnd())) } catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }")
}

pub fn run_script(
    target: &SshTarget,
    platform: HostPlatform,
    script: String,
    job: &Arc<Job>,
    askpass: &Askpass,
) -> Result<String, String> {
    if platform == HostPlatform::Windows {
        let encoded = powershell_reader();
        run_remote_command(
            target,
            script,
            job,
            askpass,
            command(target, true)?,
            &[
                "powershell.exe",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encoded,
            ],
        )
    } else {
        run_script_with_command(target, script, job, askpass, command(target, true)?)
    }
}

fn run_script_with_command(
    target: &SshTarget,
    script: String,
    job: &Arc<Job>,
    askpass: &Askpass,
    command: Command,
) -> Result<String, String> {
    run_remote_command(target, script, job, askpass, command, &["sh", "-l", "-s"])
}

/// A failed ssh to a sandbox in a Docker account whose output says the JWT was rejected:
/// drop the account's cached JWT (the shared rule, [`crate::docker_accounts`]), so the next
/// spawn, a reconnect, exchanges a fresh one.
fn note_rejection(target: &SshTarget, success: bool, output: &str) {
    let account = target
        .sandbox
        .as_ref()
        .and_then(|sandbox| sandbox.docker_account.as_deref());
    crate::docker_accounts::invalidate_if_rejected(account, success, output);
}

fn run_remote_command(
    target: &SshTarget,
    script: String,
    job: &Arc<Job>,
    askpass: &Askpass,
    mut command: Command,
    remote: &[&str],
) -> Result<String, String> {
    check_target(target)?;
    if job.cancelled.load(Ordering::Relaxed) {
        return Err("Connection cancelled".into());
    }
    askpass.configure(&mut command)?;
    match target.transport() {
        Transport::Ssh => {
            command.args(["--", &target.target]);
            command.args(remote);
        }
        Transport::Codespace(_) => {
            command.args(remote);
        }
        Transport::Sandbox(sandbox) => {
            // Everything run for a sandbox machine belongs to the Host, which
            // lives in the devcontainer (or, for a plain sandbox, the outer sandbox).
            command.args(["--", &sandbox.destination()]);
            command.arg(sandbox.exec_in_container(remote));
        }
    }
    let mut child = command.spawn().map_err(|e| spawn_error(target, e))?;
    let stdout = capture(child.stdout.take().unwrap());
    let stderr = capture(child.stderr.take().unwrap());
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(script.as_bytes()));
    let deadline = Instant::now() + Duration::from_secs(300);
    let result = loop {
        if job.cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break Err(if job.cancelled.load(Ordering::Relaxed) {
                "Connection cancelled"
            } else {
                "SSH setup timed out. Check the host's network connection and try again."
            }
            .to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error.to_string());
            }
        }
    };
    let _ = writer.join();
    let output = String::from_utf8_lossy(&stdout.join().unwrap_or_default()).to_string();
    let errors = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).to_string();
    let succeeded = result?;
    note_rejection(target, succeeded, &errors);
    if !succeeded {
        return Err(format!(
            "SSH setup failed: {}",
            errors.trim().chars().take(4000).collect::<String>()
        ));
    }
    Ok(output)
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn bootstrap_script(platform: HostPlatform) -> String {
    let template = match platform {
        HostPlatform::Unix => include_str!("remote_bootstrap.sh"),
        HostPlatform::Windows => include_str!("remote_bootstrap.ps1"),
    };
    bootstrap_script_from_template(platform, template)
}

/// Host targets the bootstrap can detect, in the order of `host_release::HOST_SHA256`.
fn pinned_sha256_lines(platform: HostPlatform) -> String {
    use super::host_release::HOST_SHA256;
    HOST_SHA256
        .iter()
        .filter(|(target, _)| target.starts_with("win32") == (platform == HostPlatform::Windows))
        .map(|(target, hash)| match platform {
            // One `case` arm per target, filled in at the template's `@@SHA256@@` line.
            HostPlatform::Unix => format!("    {target}) EXPECTED={hash} ;;"),
            HostPlatform::Windows => format!("    '{target}' = '{hash}'"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn bootstrap_script_from_template(platform: HostPlatform, template: &str) -> String {
    use super::host_release::{HOST_RELEASE_BASE, HOST_VERSION};
    let sha256 = pinned_sha256_lines(platform);
    match platform {
        // include_str! preserves checkout line endings, including Windows CRLF.
        HostPlatform::Unix => template
            .replace("\r\n", "\n")
            .replace("@@VERSION@@", &shell_quote(HOST_VERSION))
            .replace("@@RELEASE@@", &shell_quote(HOST_RELEASE_BASE))
            .replace("@@SHA256@@", &sha256)
            .replace("@@WORKER_ALIVE@@", include_str!("worker_alive.sh")),
        HostPlatform::Windows => template
            .replace("@@VERSION@@", &powershell_quote(HOST_VERSION))
            .replace("@@RELEASE@@", &powershell_quote(HOST_RELEASE_BASE))
            .replace("@@SHA256@@", &sha256)
            .replace("@@ACL@@", include_str!("windows-acl.ps1")),
    }
}

/// Bootstrap for a sandbox: the codespace flags (the Host tracks the Claude
/// token by hash and installs Claude), plus an optional prelude that is
/// prepended verbatim. The prelude is the seam for per-start secrets, which
/// reach the container on stdin only.
///
/// Unlike a codespace, a sandbox runs the Host detached (`MONOCODE_HOST_DETACHED=1`): a
/// devcontainer can have `/run/systemd/system` without systemd as PID 1, which sends
/// `service install` down a systemd path that fails. The shared template is left untouched;
/// `ensure_detached_host` from `remote_bootstrap_detached.sh` is inserted before its
/// `service install` line. Restarts need nothing extra: the template's `service uninstall` stops a detached Host.
pub fn sandbox_bootstrap_script(prelude: Option<&str>) -> String {
    sandbox_script_with(prelude, "")
}

/// One step of the cloud Host upgrade (see `remote_bootstrap.sh`): `Stage` downloads and
/// verifies the package without touching the running Host; `Swap` starts the detached worker
/// that installs it (and puts the previous Host back if the new one does not start); `Status`
/// reads how the worker is doing (and finishes or rolls back one that died); `Discard` drops
/// a staged package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpgradePhase {
    Stage,
    Swap,
    Discard,
    Status,
}
impl UpgradePhase {
    /// The value of `MONOCODE_HOST_UPGRADE_PHASE`. The detached worker of a `Swap` runs as
    /// `swapnow`.
    fn word(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Swap => "swapnow",
            Self::Discard => "discard",
            Self::Status => "status",
        }
    }
}

/// What the swap worker needs to look at the running Host once more, right before it stops it.
/// Travels on stdin only, and is exported for the worker, never written to a file.
#[derive(Clone, Copy)]
pub(crate) struct SwapCheck<'a> {
    pub device_token: &'a str,
    pub environment_id: &'a str,
    /// Names this swap; the status the app reads must carry it.
    pub nonce: &'a str,
}

/// Wraps the worker in a script that stores it, starts it detached (it survives a closed ssh
/// and an app quit) and waits until it reports its first state.
fn swap_launcher(worker: &str, check: Option<SwapCheck<'_>>) -> String {
    let exports = match check {
        Some(check) => format!(
            "export MONOCODE_CHECK_TOKEN={}\nexport MONOCODE_CHECK_ENV={}\n",
            shell_quote(check.device_token),
            shell_quote(check.environment_id)
        ),
        None => String::new(),
    };
    let alive = include_str!("worker_alive.sh");
    format!(
        r#"{exports}[ -z "${{MONOCODE_HOST_TOKEN_VAR:-}}" ] || export MONOCODE_HOST_TOKEN_VAR
set -eu
umask 077
BASE="$HOME/.monocode-host"
mkdir -p "$BASE"
{alive}
# One worker at a time: a live one owns the lock.
if [ -f "$BASE/swap.lock" ] && worker_alive "$(cat "$BASE/swap.lock" 2>/dev/null)"; then
  echo 'Another Monoptah Host update is still running on this machine.' >&2
  exit 1
fi
rm -f "$BASE/swap-status"
cat > "$BASE/swap-worker.sh" <<'MONOCODE_SWAP_WORKER_END'
{worker}
MONOCODE_SWAP_WORKER_END
if command -v setsid >/dev/null 2>&1; then
  setsid nohup sh "$BASE/swap-worker.sh" >/dev/null 2>&1 </dev/null &
else
  nohup sh "$BASE/swap-worker.sh" >/dev/null 2>&1 </dev/null &
fi
_wait=0
until [ -f "$BASE/swap-status" ]; do
  _wait=$((_wait + 1))
  [ "$_wait" -lt 15 ] || {{ echo 'The Host update did not start.' >&2; exit 1; }}
  sleep 1
done
# Nothing started from here on needs the credential.
unset MONOCODE_CHECK_TOKEN MONOCODE_CHECK_ENV
echo '{{"swap":"started"}}'
"#
    )
}

/// A step of the cloud Host upgrade in a sandbox: the connect bootstrap, with the SAME prelude
/// (a restarted Host needs the Claude credential in its environment), replacing the Host only
/// when it is older or foreign.
pub(crate) fn sandbox_upgrade_script(
    prelude: Option<&str>,
    port: u16,
    phase: UpgradePhase,
    check: Option<SwapCheck<'_>>,
) -> String {
    if phase == UpgradePhase::Swap {
        // The worker inherits the prelude's exports; it carries none itself.
        let worker = sandbox_script_with(None, &upgrade_flags(port, phase, check));
        let launcher = swap_launcher(&worker, check);
        return match prelude {
            Some(prelude) => format!("{prelude}\n{launcher}"),
            None => launcher,
        };
    }
    sandbox_script_with(prelude, &upgrade_flags(port, phase, check))
}

fn upgrade_flags(port: u16, phase: UpgradePhase, check: Option<SwapCheck<'_>>) -> String {
    let nonce = match (phase, check) {
        (UpgradePhase::Swap | UpgradePhase::Status, Some(check)) => {
            format!("MONOCODE_SWAP_NONCE={}\n", shell_quote(check.nonce))
        }
        _ => String::new(),
    };
    format!(
        "MONOCODE_HOST_UPGRADE_IF_OLDER=1\nMONOCODE_HOST_UPGRADE_PHASE={}\nMONOCODE_HOST_PORT={port}\n{nonce}",
        phase.word()
    )
}

/// `extra` lines run right after the prelude, before the bootstrap.
fn sandbox_script_with(prelude: Option<&str>, extra: &str) -> String {
    let template = include_str!("remote_bootstrap.sh").replace("\r\n", "\n");
    let helpers = include_str!("remote_bootstrap_detached.sh").replace("\r\n", "\n");
    let install = "\"$ENTRY\" service install --port \"$HOST_PORT\" >/dev/null";
    // Fail loudly at run time (also in release builds) if upstream changed the install line:
    // a script that silently skipped the detached start would only fail later on the systemd path.
    let body = if template.matches(install).count() == 1 {
        bootstrap_script_from_template(
            HostPlatform::Unix,
            &template.replace(install, &format!("ensure_detached_host\n{install}")),
        )
    } else {
        "echo 'The sandbox bootstrap template changed: the service install line is missing.' >&2\nexit 1\n".to_string()
    };
    let script = format!(
        "{extra}MONOCODE_HOST_TRACK_TOKEN=1\nMONOCODE_HOST_INSTALL_CLAUDE=1\n{helpers}{body}"
    );
    match prelude {
        Some(prelude) => format!("{prelude}\n{script}"),
        None => script,
    }
}

/// Bootstrap for a codespace without an app Claude account: the Codespaces secret
/// (CLAUDE_CODE_OAUTH_TOKEN) reaches the login shell's environment, and the host's agents only
/// inherit what the host process itself was started with. The script restarts the host once
/// when that token differs from the one the running host was started with (tracked as a hash,
/// never the token itself).
pub fn codespace_bootstrap_script() -> String {
    codespace_bootstrap_script_with(None)
}

/// [`codespace_bootstrap_script`] for a project that chooses an app Claude account: the
/// `prelude` ([`crate::cloud_sessions::docker_start::ClaudeAuth::prelude`]) is prepended
/// verbatim, so it unsets the credential variables the login shell got from the Codespaces
/// secret and exports the account's own. It reaches the codespace on stdin only. Without a
/// prelude the script is exactly [`codespace_bootstrap_script`] (nothing changes for a project
/// without an account).
pub fn codespace_bootstrap_script_with(prelude: Option<&str>) -> String {
    let script = format!(
        "MONOCODE_HOST_TRACK_TOKEN=1\nMONOCODE_HOST_INSTALL_CLAUDE=1\n{}",
        bootstrap_script(HostPlatform::Unix)
    );
    match prelude {
        Some(prelude) => format!("{prelude}\n{script}"),
        None => script,
    }
}

/// A step of the cloud Host upgrade in a codespace without an app Claude account.
#[cfg(test)]
pub(crate) fn codespace_upgrade_script(
    port: u16,
    phase: UpgradePhase,
    check: Option<SwapCheck<'_>>,
) -> String {
    codespace_upgrade_script_with(None, port, phase, check)
}

/// A step of the cloud Host upgrade in a codespace. Without a `prelude` the restarted Host
/// inherits the Claude token of the Codespaces secret from the login shell; with one (a
/// project that chooses an app Claude account) it is the SAME prelude the connect step uses,
/// like a sandbox, so an upgrade never puts the Host back on the secret.
pub(crate) fn codespace_upgrade_script_with(
    prelude: Option<&str>,
    port: u16,
    phase: UpgradePhase,
    check: Option<SwapCheck<'_>>,
) -> String {
    let script = format!(
        "{}{}",
        upgrade_flags(port, phase, check),
        codespace_bootstrap_script()
    );
    if phase == UpgradePhase::Swap {
        // The worker inherits the prelude's exports; it carries none itself.
        let launcher = swap_launcher(&script, check);
        return match prelude {
            Some(prelude) => format!("{prelude}\n{launcher}"),
            None => launcher,
        };
    }
    match prelude {
        Some(prelude) => format!("{prelude}\n{script}"),
        None => script,
    }
}

pub fn upgrade_script(platform: HostPlatform, port: u16) -> String {
    let script = bootstrap_script(platform);
    match platform {
        HostPlatform::Unix => {
            format!("MONOCODE_HOST_FORCE_UPGRADE=1\nMONOCODE_HOST_PORT={port}\n{script}")
        }
        HostPlatform::Windows => format!(
            "$env:MONOCODE_HOST_FORCE_UPGRADE = '1'\n$env:MONOCODE_HOST_PORT = '{port}'\n{script}"
        ),
    }
}

pub fn pairing_script(platform: HostPlatform, name: &str) -> String {
    match platform {
        HostPlatform::Unix => format!("set -eu\n\"$HOME/.monocode-host/bin/monocode-host\" pair --name {} --json\n", shell_quote(name)),
        HostPlatform::Windows => format!("$ErrorActionPreference = 'Stop'\n$base = Join-Path ([Environment]::GetFolderPath('UserProfile')) '.monocode-host'\n$runtime = [IO.File]::ReadAllText((Join-Path $base 'runtime-path')).Trim()\n& (Join-Path $runtime 'node.exe') (Join-Path $runtime 'host.mjs') pair --name {} --json\nif ($LASTEXITCODE -ne 0) {{ throw 'Host pairing failed.' }}\n", powershell_quote(name)),
    }
}

pub struct Tunnel {
    child: Child,
    pub port: u16,
    stderr: Arc<Mutex<String>>,
    /// Distinguishes this tunnel from every other one ever started, even on a reused port.
    id: u64,
}
static NEXT_TUNNEL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn next_tunnel_id() -> u64 {
    NEXT_TUNNEL_ID.fetch_add(1, Ordering::Relaxed)
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Tunnel {
    /// A tunnel that forwards nothing itself: `port` is wherever a test serves the Host.
    #[cfg(test)]
    pub(super) fn for_test(port: u16) -> Self {
        let child = Command::new("sleep")
            .arg("600")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            child,
            port,
            stderr: Arc::default(),
            id: next_tunnel_id(),
        }
    }
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
    pub fn start(
        target: &SshTarget,
        job: Option<&Arc<Job>>,
        askpass: Option<&Askpass>,
    ) -> Result<Self, String> {
        Self::start_with_command(target, job, askpass, command(target, askpass.is_some())?)
    }

    fn start_with_command(
        target: &SshTarget,
        job: Option<&Arc<Job>>,
        askpass: Option<&Askpass>,
        mut command: Command,
    ) -> Result<Self, String> {
        check_target(target)?;
        let port = reserve_local_port()?;
        if let Some(askpass) = askpass {
            askpass.configure(&mut command)?;
        }
        // A sandbox Host listens inside the container, so the tunnel ends at
        // the relay in the outer sandbox (the only place `-L` may point to). A plain
        // sandbox runs the Host in the outer sandbox itself: no relay.
        let remote_port = match target.transport() {
            Transport::Sandbox(sandbox) if sandbox.mode == SandboxMode::Container => {
                sandbox.relay_port
            }
            Transport::Sandbox(_) => target.remote_port,
            Transport::Ssh | Transport::Codespace(_) => target.remote_port,
        };
        command.args([
            "-N",
            "-L",
            &format!("127.0.0.1:{port}:127.0.0.1:{remote_port}"),
        ]);
        match target.transport() {
            Transport::Ssh => {
                command.args(["--", &target.target]);
            }
            Transport::Codespace(_) => {}
            Transport::Sandbox(sandbox) => {
                command.args(["--", &sandbox.destination()]);
            }
        }
        command.stdin(Stdio::null()).stdout(Stdio::null());
        let mut child = command.spawn().map_err(|e| spawn_error(target, e))?;
        let mut reader = child.stderr.take().unwrap();
        let stderr = Arc::new(Mutex::new(String::new()));
        let errors = stderr.clone();
        std::thread::spawn(move || {
            let mut buffer = [0; 2048];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let mut errors = errors.lock().unwrap();
                if errors.len() < 8192 {
                    errors.push_str(&String::from_utf8_lossy(&buffer[..count]));
                }
            }
        });
        let mut tunnel = Self {
            child,
            port,
            stderr,
            id: next_tunnel_id(),
        };
        // gh starts a stopped codespace before it can forward, and a sandbox
        // resumes on connect, which takes a while.
        let patient = job.is_some() || target.is_cloud();
        let deadline = Instant::now() + Duration::from_secs(if patient { 150 } else { 20 });
        loop {
            if job.is_some_and(|j| j.cancelled.load(Ordering::Relaxed)) {
                return Err("Connection cancelled".into());
            }
            if !tunnel.alive() {
                let errors = tunnel.stderr.lock().unwrap().clone();
                note_rejection(target, false, &errors);
                return Err(format!(
                    "SSH connection failed: {}. Open Settings → Connections and reconnect to check access.",
                    errors.trim()
                ));
            }
            if port_accepts(port) {
                return Ok(tunnel);
            }
            if Instant::now() >= deadline {
                return Err(
                    "SSH timed out. Open Settings → Connections and reconnect to authenticate."
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Pick a free local port for the tunnel to bind. Binding port 0 and dropping
/// the listener is not enough on its own: where sockets are not created with
/// close-on-exec atomically (macOS), a child spawned on another thread at that
/// moment inherits the listener and keeps the port accepting connections after
/// we closed ours. The readiness probe would then take that stray listener for
/// the tunnel, and the first request would hit a connection that is reset or
/// refused once the child exits. A port that still accepts is skipped.
fn reserve_local_port() -> Result<u16, String> {
    for _ in 0..20 {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener);
        if !port_accepts(port) {
            return Ok(port);
        }
    }
    Err("No free local port is available for the SSH tunnel".into())
}

fn port_accepts(port: u16) -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(100)).is_ok()
}

#[derive(Default)]
struct Slot {
    tunnel: Option<Tunnel>,
    failure: Option<(Instant, String)>,
    generation: u64,
}

pub struct TunnelLease {
    pub endpoint: String,
    /// The tunnel this lease is on (see `Tunnel::id`).
    pub tunnel_id: u64,
    slot: Arc<Mutex<Slot>>,
    generation: u64,
    /// The tunnel was started for this lease (not reused).
    pub fresh: bool,
}

/// Each machine has its own lock: restarting one machine's tunnel (up to
/// 20 seconds for plain SSH) must not block requests to other machines.
/// Codespace machines never restart lazily (that would wake the codespace):
/// they use `existing`, which only looks, and are reconnected explicitly.
#[derive(Default)]
pub struct Tunnels {
    slots: Mutex<HashMap<String, Arc<Mutex<Slot>>>>,
}
impl Tunnels {
    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Mutex<Slot>>>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub fn insert(&self, id: String, tunnel: Tunnel) {
        // A fresh slot never waits for a reconnect in progress; that attempt's
        // tunnel is dropped with the replaced slot.
        let slot = Slot {
            tunnel: Some(tunnel),
            failure: None,
            generation: 1,
        };
        let old = self.slots().insert(id, Arc::new(Mutex::new(slot)));
        drop(old);
    }
    pub fn remove(&self, id: &str) {
        let old = self.slots().remove(id);
        drop(old);
    }
    /// A live tunnel for `id`, or None. Never spawns a process and holds the
    /// machine lock only for a liveness check.
    pub fn existing(&self, id: &str) -> Option<TunnelLease> {
        let slot = self.slots().get(id).cloned()?;
        let mut current = slot.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, tunnel_id) = {
            let tunnel = current.tunnel.as_mut()?;
            if !tunnel.alive() {
                current.tunnel = None;
                return None;
            }
            (tunnel.port, tunnel.id)
        };
        Some(TunnelLease {
            endpoint: format!("http://127.0.0.1:{port}"),
            tunnel_id,
            slot: slot.clone(),
            generation: current.generation,
            fresh: false,
        })
    }
    pub fn endpoint(&self, id: &str, target: &SshTarget) -> Result<TunnelLease, String> {
        let slot = self.slots().entry(id.into()).or_default().clone();
        let mut current = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(tunnel) = current.tunnel.as_mut() {
            if tunnel.alive() {
                return Ok(TunnelLease {
                    endpoint: format!("http://127.0.0.1:{}", tunnel.port),
                    tunnel_id: tunnel.id,
                    slot: slot.clone(),
                    generation: current.generation,
                    fresh: false,
                });
            }
        }
        current.tunnel = None;
        if let Some((when, error)) = &current.failure {
            if when.elapsed() < Duration::from_secs(10) {
                return Err(error.clone());
            }
        }
        match Tunnel::start(target, None, None) {
            Ok(tunnel) => {
                let endpoint = format!("http://127.0.0.1:{}", tunnel.port);
                let tunnel_id = tunnel.id;
                current.failure = None;
                current.generation = current.generation.wrapping_add(1);
                current.tunnel = Some(tunnel);
                Ok(TunnelLease {
                    endpoint,
                    tunnel_id,
                    slot: slot.clone(),
                    generation: current.generation,
                    fresh: true,
                })
            }
            Err(error) => {
                current.failure = Some((Instant::now(), error.clone()));
                Err(error)
            }
        }
    }
    pub fn invalidate(&self, id: &str, lease: &TunnelLease) {
        let Some(slot) = self.slots().get(id).cloned() else {
            return;
        };
        if !Arc::ptr_eq(&slot, &lease.slot) {
            return;
        }
        let mut current = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if current.generation == lease.generation {
            current.tunnel = None;
            current.failure = None;
        }
    }
    pub fn clear(&self) {
        let old = std::mem::take(&mut *self.slots());
        drop(old);
    }
}

/// How this desktop appears in the host's device list.
pub fn device_name() -> String {
    #[cfg(not(windows))]
    let run = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    #[cfg(target_os = "macos")]
    let name = run("scutil", &["--get", "ComputerName"]).or_else(|| run("hostname", &[]));
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(any(target_os = "macos", windows)))]
    let name = std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| run("hostname", &[]));
    let name: String = name
        .unwrap_or_default()
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    if name.is_empty() {
        "Monoptah desktop".into()
    } else {
        format!("Monoptah on {name}")
    }
}

#[cfg(test)]
mod bootstrap_libc_tests;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_ports_are_not_accepting_connections() {
        let port = reserve_local_port().unwrap();
        assert!(!port_accepts(port));
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        assert!(port_accepts(port));
        drop(listener);
    }

    #[test]
    fn stale_requests_cannot_invalidate_a_newer_tunnel() {
        let tunnels = Tunnels::default();
        let slot = |generation| {
            Arc::new(Mutex::new(Slot {
                tunnel: None,
                failure: Some((Instant::now(), "keep".into())),
                generation,
            }))
        };
        let old = slot(1);
        tunnels.slots().insert("host".into(), old.clone());
        let old_lease = TunnelLease {
            endpoint: String::new(),
            tunnel_id: 0,
            slot: old,
            generation: 1,
            fresh: false,
        };
        let newer = slot(2);
        tunnels.slots().insert("host".into(), newer.clone());
        tunnels.invalidate("host", &old_lease);
        assert!(newer.lock().unwrap().failure.is_some());
        let stale_lease = TunnelLease {
            endpoint: String::new(),
            tunnel_id: 0,
            slot: newer.clone(),
            generation: 1,
            fresh: false,
        };
        tunnels.invalidate("host", &stale_lease);
        assert!(newer.lock().unwrap().failure.is_some());
        tunnels.invalidate(
            "host",
            &TunnelLease {
                generation: 2,
                ..stale_lease
            },
        );
        assert!(newer.lock().unwrap().failure.is_none());
    }
    // scripts/test-remote-ssh.py creates an isolated sshd, host and keypair.
    // This test uses the production tunnel lifecycle and shell transport.
    #[test]
    #[ignore = "requires the isolated loopback SSH fixture"]
    fn loopback_transport_preserves_host_and_reconnects() {
        let required = |key| std::env::var(key).expect("Run scripts/test-remote-ssh.py");
        let target = SshTarget {
            target: required("MONOCODE_TEST_SSH_TARGET"),
            port: Some(required("MONOCODE_TEST_SSH_PORT").parse().unwrap()),
            remote_port: required("MONOCODE_TEST_HOST_PORT").parse().unwrap(),
            codespace: None,
            sandbox: None,
        };
        let make_command = || {
            let mut command = command(&target, false).unwrap();
            command.args([
                "-F",
                "/dev/null",
                "-i",
                &required("MONOCODE_TEST_SSH_KEY"),
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                &format!(
                    "UserKnownHostsFile={}",
                    required("MONOCODE_TEST_KNOWN_HOSTS")
                ),
            ]);
            command
        };
        let job = Job::new();
        let askpass = job.askpass().unwrap();
        let detected = run_remote_command(
            &target,
            String::new(),
            &job,
            &askpass,
            make_command(),
            PLATFORM_PROBE,
        )
        .unwrap();
        assert_eq!(parse_platform(&detected).unwrap(), HostPlatform::Unix);
        let output = run_script_with_command(
            &target,
            "printf 'remote-script-ok\\n'\n".into(),
            &job,
            &askpass,
            make_command(),
        )
        .unwrap();
        assert_eq!(output.trim(), "remote-script-ok");
        let environment = required("MONOCODE_TEST_ENVIRONMENT");
        for _ in 0..2 {
            let tunnel = Tunnel::start_with_command(&target, None, None, make_command()).unwrap();
            let response = ureq::post(&format!("http://127.0.0.1:{}/rpc", tunnel.port))
                .set(
                    "Authorization",
                    &format!("Bearer {}", required("MONOCODE_TEST_TOKEN")),
                )
                .send_string(r#"{"version":1,"method":"environment.describe"}"#)
                .unwrap();
            let value: serde_json::Value = serde_json::from_reader(response.into_reader()).unwrap();
            assert_eq!(value["result"]["environmentId"], environment);
            drop(tunnel);
            // A client disappearing must not kill the independently owned host.
            assert!(TcpStream::connect(("127.0.0.1", target.remote_port)).is_ok());
        }
    }
    fn codespace_target(name: &str) -> SshTarget {
        SshTarget {
            target: name.into(),
            port: None,
            remote_port: 3774,
            codespace: Some(name.into()),
            sandbox: None,
        }
    }
    fn env_of(command: &Command, key: &str) -> Option<String> {
        command
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new(key))
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }
    fn argv(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
    #[test]
    fn codespace_commands_use_gh_without_a_destination_or_host_key_prompts() {
        let command =
            command_with_program(&codespace_target("octo-abc123"), true, "gh".as_ref()).unwrap();
        assert_eq!(command.get_program(), "gh");
        let expected: Vec<&str> = vec![
            "codespace",
            "ssh",
            "-c",
            "octo-abc123",
            "--",
            "-T",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ForwardX11=no",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ForkAfterAuthentication=no",
            "-o",
            "BatchMode=yes",
        ];
        assert_eq!(argv(&command), expected);
    }
    #[test]
    fn plain_ssh_commands_are_unchanged() {
        let target = SshTarget {
            target: "me@box".into(),
            port: Some(2222),
            remote_port: 3774,
            codespace: None,
            sandbox: None,
        };
        let command = command_with_program(&target, true, "ssh".as_ref()).unwrap();
        let args = argv(&command);
        assert_eq!(command.get_program(), "ssh");
        assert!(args.contains(&"StrictHostKeyChecking=ask".to_string()));
        assert!(args.contains(&"BatchMode=no".to_string()));
        assert_eq!(&args[args.len() - 2..], ["-p", "2222"]);
        assert!(!args.contains(&"codespace".to_string()));
    }
    #[test]
    fn remote_commands_to_a_codespace_carry_no_target_separator() {
        let job = Job::new();
        let askpass = job.askpass().unwrap();
        let echo = |target: &SshTarget| {
            let mut command = Command::new("echo");
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            run_remote_command(
                target,
                String::new(),
                &job,
                &askpass,
                command,
                &["sh", "-l", "-s"],
            )
            .unwrap()
        };
        assert_eq!(echo(&codespace_target("octo-abc123")).trim(), "sh -l -s");
        let plain = SshTarget {
            target: "me@box".into(),
            port: None,
            remote_port: 3774,
            codespace: None,
            sandbox: None,
        };
        assert_eq!(echo(&plain).trim(), "-- me@box sh -l -s");
    }
    #[test]
    fn codespace_names_cannot_inject_options_or_shell_commands() {
        for name in ["octo-abc123", "A1", " padded-name "] {
            assert!(validate_codespace(name).is_ok(), "{name}");
        }
        assert_eq!(validate_codespace(" padded-name ").unwrap(), "padded-name");
        for name in [
            "",
            "-oProxyCommand=bad",
            "a b",
            "a;b",
            "a/b",
            "a_b",
            "na\nme",
            "$(whoami)",
            "a@b",
            &"a".repeat(101),
        ] {
            assert!(validate_codespace(name).is_err(), "{name}");
        }
    }
    #[test]
    fn stored_targets_without_a_codespace_still_load() {
        let old: SshTarget =
            serde_json::from_str(r#"{"target":"me@box","port":22,"remotePort":3774}"#).unwrap();
        assert_eq!(old.codespace, None);
        // Plain targets keep their old shape on disk.
        assert!(!serde_json::to_string(&old).unwrap().contains("codespace"));
        let new: SshTarget =
            serde_json::from_str(r#"{"target":"x","port":null,"remotePort":3774,"codespace":"x"}"#)
                .unwrap();
        assert_eq!(new, codespace_target("x"));
    }
    #[test]
    fn codespace_platform_is_unix_without_probing() {
        let job = Job::new();
        let askpass = job.askpass().unwrap();
        assert_eq!(
            detect_platform(&codespace_target("x"), &job, &askpass).unwrap(),
            HostPlatform::Unix
        );
    }
    #[test]
    fn codespace_bootstrap_tracks_the_token_without_exposing_it() {
        let script = codespace_bootstrap_script();
        assert!(script
            .starts_with("MONOCODE_HOST_TRACK_TOKEN=1\nMONOCODE_HOST_INSTALL_CLAUDE=1\nset -eu\n"));
        assert!(!bootstrap_script(HostPlatform::Unix).contains("MONOCODE_HOST_TRACK_TOKEN=1\n"));
        assert!(script.contains("host-token.sha256"));
        assert!(!script.contains("echo \"$CLAUDE_CODE_OAUTH_TOKEN"));
    }
    #[cfg(unix)]
    #[test]
    fn codespace_bootstrap_restarts_the_host_once_when_the_token_changes() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".monocode-host/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = home.path().join("calls.log");
        let entry = bin.join("monocode-host");
        std::fs::write(
            &entry,
            format!(
                "#!/bin/sh\necho \"$1 $2\" >> {}\n[ \"$1\" = connection-info ] && echo '{{\"port\":3774}}'\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |script: String, token: Option<&str>| {
            let mut command = Command::new("sh");
            command
                .arg("-s")
                .env("HOME", home.path())
                .env_remove("CLAUDE_CODE_OAUTH_TOKEN");
            if let Some(token) = token {
                command.env("CLAUDE_CODE_OAUTH_TOKEN", token);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-token"));
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        // Hermetic: never try to install or detect Claude Code from this test.
        let no_claude_install = || {
            codespace_bootstrap_script().replace(
                "MONOCODE_HOST_INSTALL_CLAUDE=1",
                "MONOCODE_HOST_INSTALL_CLAUDE=0",
            )
        };
        let uninstalls =
            |calls: &[String]| calls.iter().filter(|c| *c == "service uninstall").count();
        // No token in the shell: never restart.
        assert_eq!(uninstalls(&run(no_claude_install(), None)), 0);
        // Token appears: restart once, store a hash only.
        let calls = run(no_claude_install(), Some("secret-token"));
        assert_eq!(uninstalls(&calls), 1, "{calls:?}");
        let marker =
            std::fs::read_to_string(home.path().join(".monocode-host/host-token.sha256")).unwrap();
        assert_eq!(marker.trim().len(), 64);
        assert!(!marker.contains("secret-token"));
        // Same token: no further restart.
        assert_eq!(
            uninstalls(&run(no_claude_install(), Some("secret-token"))),
            1
        );
        // Changed token: restart again.
        assert_eq!(
            uninstalls(&run(no_claude_install(), Some("other-token"))),
            2
        );
        // Plain ssh bootstrap ignores the token entirely.
        assert_eq!(
            uninstalls(&run(bootstrap_script(HostPlatform::Unix), Some("third"))),
            2
        );
    }
    /// The sandbox prelude may pin an API key instead of a setup token; the bootstrap then
    /// tracks that variable, and a setup token leaves exactly the marker a codespace writes.
    #[cfg(unix)]
    #[test]
    fn sandbox_bootstrap_tracks_the_pinned_variable_with_the_codespace_marker() {
        use std::os::unix::fs::PermissionsExt;
        let run = |script: String, env: Option<(&str, &str)>| -> (Vec<String>, String) {
            let home = tempfile::tempdir().unwrap();
            let bin = home.path().join(".monocode-host/bin");
            std::fs::create_dir_all(&bin).unwrap();
            let log = home.path().join("calls.log");
            let entry = bin.join("monocode-host");
            std::fs::write(
                &entry,
                format!(
                    "#!/bin/sh\necho \"$1 $2\" >> {}\n[ \"$1\" = connection-info ] && echo '{{\"port\":3774}}'\nexit 0\n",
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mut command = Command::new("sh");
            command
                .arg("-s")
                .env("HOME", home.path())
                .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
                .env_remove("ANTHROPIC_API_KEY");
            if let Some((name, value)) = env {
                command.env(name, value);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let calls = std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(String::from)
                .collect();
            let marker =
                std::fs::read_to_string(home.path().join(".monocode-host/host-token.sha256"))
                    .unwrap_or_default();
            (calls, marker)
        };
        let hermetic = |script: String| {
            script.replace(
                "MONOCODE_HOST_INSTALL_CLAUDE=1",
                "MONOCODE_HOST_INSTALL_CLAUDE=0",
            )
        };
        let (_, codespace_marker) = run(
            hermetic(codespace_bootstrap_script()),
            Some(("CLAUDE_CODE_OAUTH_TOKEN", "same-secret")),
        );
        assert_eq!(codespace_marker.trim().len(), 64);
        // A sandbox with a setup token writes the very same marker.
        let token_prelude = "unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\nexport CLAUDE_CODE_OAUTH_TOKEN='same-secret'";
        let (calls, marker) = run(
            hermetic(sandbox_bootstrap_script(Some(token_prelude))),
            None,
        );
        assert_eq!(marker, codespace_marker);
        assert_eq!(
            calls.iter().filter(|c| *c == "service uninstall").count(),
            1
        );
        // An API key is tracked through its own variable and hashes the same way.
        let key_prelude = "unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\nMONOCODE_HOST_TOKEN_VAR=ANTHROPIC_API_KEY\nexport ANTHROPIC_API_KEY='same-secret'";
        let (calls, marker) = run(hermetic(sandbox_bootstrap_script(Some(key_prelude))), None);
        assert_eq!(marker, codespace_marker);
        assert_eq!(
            calls.iter().filter(|c| *c == "service uninstall").count(),
            1
        );
        // Without the selector an API key in the environment is not tracked (codespace rule).
        let (calls, marker) = run(
            hermetic(codespace_bootstrap_script()),
            Some(("ANTHROPIC_API_KEY", "same-secret")),
        );
        assert!(marker.is_empty());
        assert_eq!(
            calls.iter().filter(|c| *c == "service uninstall").count(),
            0
        );
    }
    #[cfg(unix)]
    #[test]
    fn codespace_bootstrap_restarts_the_host_once_when_claude_appears() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".monocode-host/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = home.path().join("calls.log");
        let entry = bin.join("monocode-host");
        std::fs::write(
            &entry,
            format!(
                "#!/bin/sh\necho \"$1 $2\" >> {}\n[ \"$1\" = connection-info ] && echo '{{\"port\":3774}}'\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A claude binary already in ~/.local/bin: nothing is downloaded.
        let local = home.path().join(".local/bin");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("claude"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(local.join("claude"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let run = || {
            let mut child = Command::new("sh")
                .arg("-s")
                .env("HOME", home.path())
                .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(codespace_bootstrap_script().as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .filter(|c| *c == "service uninstall")
                .count()
        };
        // The host may have started before Claude existed: restart once, then never again.
        assert_eq!(run(), 1);
        assert_eq!(run(), 1);
    }

    #[test]
    fn ssh_targets_cannot_inject_options_or_shell_commands() {
        for target in [
            "home",
            "me@mac-mini.local",
            "user@192.168.1.4",
            "user@[::1]",
        ] {
            assert!(validate_target(target, None).is_ok());
        }
        for target in [
            "",
            "-oProxyCommand=bad",
            "host;touch /tmp/x",
            "host\nname",
            "$(whoami)",
            "user@host command",
            "host/../../x",
            "ssh://user@host",
            "a@b@c",
        ] {
            assert!(validate_target(target, None).is_err(), "{target}");
        }
        assert!(validate_target("host", Some(0)).is_err());
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
    #[test]
    fn unix_bootstrap_accepts_windows_checkout_line_endings() {
        let lf_template = include_str!("remote_bootstrap.sh").replace("\r\n", "\n");
        let crlf_template = lf_template.replace('\n', "\r\n");
        let script = bootstrap_script_from_template(HostPlatform::Unix, &crlf_template);
        assert!(script.starts_with("set -eu\n"));
        assert!(!script.contains('\r'));
        assert_eq!(
            script,
            bootstrap_script_from_template(HostPlatform::Unix, &lf_template)
        );
    }
    #[test]
    fn bootstrap_is_versioned_and_only_explicit_upgrade_restarts_the_host() {
        let script = bootstrap_script(HostPlatform::Unix);
        assert!(!script.contains('\r'));
        assert!(!script.contains("@@"));
        assert!(script.contains("--proto '=https'"));
        assert!(script.contains("checksum mismatch"));
        assert!(script.contains("\"$FORCE_UPGRADE\" = 1"));
        assert!(script.contains("service uninstall"));
        assert!(
            upgrade_script(HostPlatform::Unix, 3774).starts_with("MONOCODE_HOST_FORCE_UPGRADE=1")
        );
        assert!(!upgrade_script(HostPlatform::Unix, 3774).contains('\r'));
        assert!(upgrade_script(HostPlatform::Windows, 3774)
            .starts_with("$env:MONOCODE_HOST_FORCE_UPGRADE = '1'"));
    }
    #[test]
    fn bootstrap_pins_the_fork_release_version_and_hashes() {
        use super::super::host_release::{HOST_RELEASE_BASE, HOST_SHA256, HOST_VERSION};
        let unix = bootstrap_script(HostPlatform::Unix);
        assert!(unix.contains(&format!("RELEASE={}\n", shell_quote(HOST_RELEASE_BASE))));
        assert!(unix.contains(&format!("VERSION={}\n", shell_quote(HOST_VERSION))));
        assert!(HOST_RELEASE_BASE
            .starts_with("https://github.com/hhoangg/monoptah/releases/download/host-v"));
        assert!(HOST_VERSION.ends_with("+monoptah"));
        assert!(!unix.contains("hardbeat920"));
        for (target, hash) in HOST_SHA256 {
            assert_eq!(
                unix.contains(&format!("    {target}) EXPECTED={hash} ;;")),
                !target.starts_with("win32"),
                "{target}"
            );
        }
        let windows = bootstrap_script(HostPlatform::Windows);
        assert!(windows.contains(&format!(
            "$release = {}",
            powershell_quote(HOST_RELEASE_BASE)
        )));
        assert!(windows.contains(&format!("$version = {}", powershell_quote(HOST_VERSION))));
        for (target, hash) in HOST_SHA256 {
            assert_eq!(
                windows.contains(&format!("'{target}' = '{hash}'")),
                target.starts_with("win32"),
                "{target}"
            );
        }
        // The reinstall rule is unchanged: missing or FORCE_UPGRADE.
        assert!(unix.contains("[ ! -x \"$ENTRY\" ] || [ \"$FORCE_UPGRADE\" = 1 ]"));
    }
    /// Runs the real Unix bootstrap with a stub `curl` that serves one fake package, and
    /// returns (exit success, stderr, urls requested, HOME).
    #[cfg(unix)]
    fn run_bootstrap_with_fake_package(
        pin_hash_of_package: bool,
        publish: Option<&str>,
        tamper: bool,
    ) -> (bool, String, Vec<String>, tempfile::TempDir) {
        run_bootstrap_on(
            pin_hash_of_package,
            publish,
            tamper,
            None,
            bootstrap_script(HostPlatform::Unix),
        )
    }
    /// [`run_bootstrap_with_fake_package`] for any script, with an already installed Host
    /// that reports `installed` from `--version`. Every call to a Host is logged in
    /// `$HOME/calls.log`.
    #[cfg(unix)]
    fn run_bootstrap_on(
        pin_hash_of_package: bool,
        publish: Option<&str>,
        tamper: bool,
        installed: Option<&str>,
        mut script: String,
    ) -> (bool, String, Vec<String>, tempfile::TempDir) {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::PermissionsExt;
        let work = tempfile::tempdir().unwrap();
        let pkg = work.path().join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        let host = pkg.join("monocode-host");
        std::fs::write(
            &host,
            "#!/bin/sh\necho \"new $@\" >> \"$HOME/calls.log\"\nenv >> \"$HOME/env.log\"\ncase \"$1\" in --version) echo 'PINNED_VERSION' ;; connection-info) if [ -f \"$HOME/flaky\" ]; then n=$(cat \"$HOME/flaky\"); if [ \"$n\" -gt 0 ]; then echo $((n - 1)) > \"$HOME/flaky\"; exit 1; fi; fi; [ ! -f \"$HOME/fail-new\" ] || exit 1; echo '{\"port\":3774}' ;; service) [ \"$2\" != install ] || [ ! -f \"$HOME/fail-new\" ] || exit 1 ;; esac\nexit 0\n"
            .replace("PINNED_VERSION", super::super::host_release::HOST_VERSION),
        )
        .unwrap();
        std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tarball = work.path().join("pkg.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&tarball)
            .arg("-C")
            .arg(&pkg)
            .arg("monocode-host")
            .status()
            .unwrap();
        assert!(status.success());
        let package_hash = format!("{:x}", Sha256::digest(std::fs::read(&tarball).unwrap()));
        // The checksum the "release" publishes next to the package.
        let published = publish
            .map(String::from)
            .unwrap_or_else(|| package_hash.clone());
        std::fs::write(
            work.path().join("pkg.sha256"),
            format!("{published}  pkg\n"),
        )
        .unwrap();
        if tamper {
            // Serve other bytes than the ones that were hashed.
            std::fs::write(&tarball, b"not the package").unwrap();
        }
        let stubs = work.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        let curl = stubs.join("curl");
        std::fs::write(
            &curl,
            format!(
                "#!/bin/sh\nurl=; out=\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=$2; shift ;; https://*) url=$1 ;; esac; shift; done\necho \"$url\" >> {log}\ncase \"$url\" in *.sha256) cp {sha} \"$out\" ;; *) cp {tar} \"$out\" ;; esac\n",
                log = work.path().join("urls.log").display(),
                sha = work.path().join("pkg.sha256").display(),
                tar = tarball.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        if pin_hash_of_package {
            for (_, hash) in super::super::host_release::HOST_SHA256 {
                script = script.replace(hash, &package_hash);
            }
        }
        let home = tempfile::tempdir().unwrap();
        if let Some(version) = installed {
            install_old_host(home.path(), version);
        }
        let mut child = Command::new("sh")
            .arg("-s")
            .env("HOME", home.path())
            .env(
                "PATH",
                format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap()),
            )
            .env_remove("MONOCODE_HOST_FORCE_UPGRADE")
            .env_remove("MONOCODE_HOST_UPGRADE_IF_OLDER")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let urls = std::fs::read_to_string(work.path().join("urls.log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            urls,
            home,
        )
    }
    #[cfg(unix)]
    #[test]
    fn bootstrap_installs_a_package_that_matches_the_pinned_hash() {
        let (ok, stderr, urls, home) = run_bootstrap_with_fake_package(true, None, false);
        assert!(ok, "{stderr}");
        assert_eq!(urls.len(), 2, "{urls:?}");
        for url in &urls {
            assert!(
                url.starts_with("https://github.com/hhoangg/monoptah/releases/download/host-v"),
                "{url}"
            );
        }
        assert!(home
            .path()
            .join(".monocode-host/bin/monocode-host")
            .is_file());
    }
    #[cfg(unix)]
    #[test]
    fn bootstrap_rejects_a_wrong_pinned_hash_and_a_tampered_package() {
        // The pin is the real release hash: the fake package can not match it.
        let (ok, stderr, _, home) = run_bootstrap_with_fake_package(false, None, false);
        assert!(!ok);
        assert!(stderr.contains("checksum mismatch"), "{stderr}");
        assert!(!home
            .path()
            .join(".monocode-host/bin/monocode-host")
            .exists());
        // Pin and published checksum agree, but the served bytes differ.
        let (ok, stderr, _, home) = run_bootstrap_with_fake_package(true, None, true);
        assert!(!ok);
        assert!(stderr.contains("package checksum mismatch"), "{stderr}");
        assert!(!home
            .path()
            .join(".monocode-host/bin/monocode-host")
            .exists());
        // The pin matches the package, but the release publishes another checksum.
        let other = "0".repeat(64);
        let (ok, stderr, _, _) = run_bootstrap_with_fake_package(true, Some(&other), false);
        assert!(!ok);
        assert!(stderr.contains("pinned checksum mismatch"), "{stderr}");
    }
    /// An installed Host as the bootstrap leaves it: a versioned runtime that reports `version`,
    /// `runtime-path` and the generic launcher. Every call it gets is logged as `old ...`.
    #[cfg(unix)]
    fn install_old_host(home: &std::path::Path, version: &str) {
        use std::os::unix::fs::PermissionsExt;
        let base = home.join(".monocode-host");
        let runtime = base.join("runtime/old");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(base.join("bin")).unwrap();
        let host = runtime.join("monocode-host");
        std::fs::write(
            &host,
            format!("#!/bin/sh\necho \"old $@\" >> \"$HOME/calls.log\"\nenv >> \"$HOME/env.log\"\ncase \"$1\" in --version) echo '{version}' ;; connection-info) if [ -f \"$HOME/flaky\" ]; then n=$(cat \"$HOME/flaky\"); if [ \"$n\" -gt 0 ]; then echo $((n - 1)) > \"$HOME/flaky\"; exit 1; fi; fi; echo '{{\"port\":3774}}' ;; service) [ \"$2\" != install ] || [ ! -f \"$HOME/fail-old\" ] || exit 1 ;; esac\nexit 0\n"),
        )
        .unwrap();
        std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            base.join("runtime-path"),
            format!("{}\n", runtime.display()),
        )
        .unwrap();
        let launcher = base.join("bin/monocode-host");
        std::fs::write(
            &launcher,
            "#!/bin/sh\nset -eu\nBASE=$(CDPATH= cd -- \"$(dirname -- \"$0\")/..\" && pwd)\nRUNTIME=$(cat \"$BASE/runtime-path\")\nexec \"$RUNTIME/monocode-host\" \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    /// Runs a script in an existing HOME with extra environment. `PATH` can be replaced.
    #[cfg(unix)]
    fn run_in_home_env(
        home: &std::path::Path,
        script: &str,
        envs: &[(&str, &str)],
        close_output: bool,
    ) -> (Option<i32>, String, String) {
        let mut command = Command::new("sh");
        command
            .arg("-s")
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        if close_output {
            // The ssh the worker was started over is gone: writing to it would break the pipe.
            drop(child.stdout.take());
            drop(child.stderr.take());
        }
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
    #[cfg(unix)]
    fn run_in_home(home: &std::path::Path, script: &str) -> (Option<i32>, String, String) {
        run_in_home_env(home, script, &[], false)
    }
    /// The cloud upgrade step in front of the plain bootstrap (no Claude install, which would
    /// call the stub `curl`), for a Host that listens on `port`.
    #[cfg(unix)]
    fn upgrade_step_on(port: u16, phase: UpgradePhase) -> String {
        format!(
            "{}{}",
            upgrade_flags(port, phase, None),
            bootstrap_script(HostPlatform::Unix)
        )
    }
    #[cfg(unix)]
    fn upgrade_step(phase: UpgradePhase) -> String {
        upgrade_step_on(3774, phase)
    }
    #[cfg(unix)]
    fn host_calls(home: &tempfile::TempDir) -> String {
        std::fs::read_to_string(home.path().join("calls.log")).unwrap_or_default()
    }
    #[cfg(unix)]
    fn runtime_path(home: &std::path::Path) -> String {
        std::fs::read_to_string(home.join(".monocode-host/runtime-path"))
            .unwrap()
            .trim()
            .to_string()
    }
    #[cfg(unix)]
    fn swap_status(home: &std::path::Path) -> String {
        std::fs::read_to_string(home.join(".monocode-host/swap-status"))
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    }
    /// The second line of the status file: the nonce of the swap that wrote it.
    #[cfg(unix)]
    fn status_nonce(home: &std::path::Path) -> String {
        std::fs::read_to_string(home.join(".monocode-host/swap-status"))
            .unwrap_or_default()
            .lines()
            .nth(1)
            .unwrap_or("")
            .to_string()
    }
    /// The status script, asking for the swap named `nonce`.
    #[cfg(unix)]
    fn status_script(nonce: &str) -> String {
        let check = SwapCheck {
            device_token: "unused",
            environment_id: "unused",
            nonce,
        };
        format!(
            "{}{}",
            upgrade_flags(3774, UpgradePhase::Status, Some(check)),
            bootstrap_script(HostPlatform::Unix)
        )
    }
    /// A process whose command line is a swap worker's, alive for a few seconds.
    #[cfg(unix)]
    fn fake_worker() -> std::process::Child {
        Command::new("sh")
            .args(["-c", "sleep 4", "swap-worker.sh"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
    #[cfg(unix)]
    fn env_log(home: &tempfile::TempDir) -> String {
        std::fs::read_to_string(home.path().join("env.log")).unwrap_or_default()
    }
    /// The Host's own API as the swap worker's last look sees it: `projects.list` has one
    /// project, `sessions.list` answers `sessions` (a JSON array body).
    #[cfg(unix)]
    struct FakeApi {
        port: u16,
        sessions: Arc<Mutex<String>>,
        stop: Arc<std::sync::atomic::AtomicBool>,
    }
    #[cfg(unix)]
    const API_TOKEN: &str = "device-token-0123456789-abcdefghijklmnopq";
    #[cfg(unix)]
    impl FakeApi {
        fn start(sessions: &str) -> Self {
            use std::io::BufRead;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let sessions = Arc::new(Mutex::new(sessions.to_string()));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (state, halt) = (sessions.clone(), stop.clone());
            std::thread::spawn(move || {
                while !halt.load(Ordering::Relaxed) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                    let (mut length, mut authorized) = (0, false);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        let lower = line.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                        authorized |= line.trim() == format!("Authorization: Bearer {API_TOKEN}")
                            || line.trim() == format!("authorization: Bearer {API_TOKEN}");
                    }
                    let mut body = vec![0; length];
                    let _ = reader.read_exact(&mut body);
                    let body = String::from_utf8_lossy(&body).into_owned();
                    let reply = if !authorized {
                        "{\"error\":\"unauthorized\"}".to_string()
                    } else if body.contains("projects.list") {
                        "{\"result\":[{\"id\":\"p1\",\"cwd\":\"/x\",\"name\":\"x\"}]}".to_string()
                    } else {
                        format!("{{\"result\":{}}}", state.lock().unwrap())
                    };
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", reply.len(), reply);
                }
            });
            Self {
                port,
                sessions,
                stop,
            }
        }
        fn set(&self, sessions: &str) {
            *self.sessions.lock().unwrap() = sessions.to_string();
        }
        fn envs(&self) -> Vec<(&'static str, &'static str)> {
            vec![
                ("MONOCODE_CHECK_TOKEN", API_TOKEN),
                ("MONOCODE_CHECK_ENV", "env-1"),
            ]
        }
    }
    #[cfg(unix)]
    impl Drop for FakeApi {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }
    #[cfg(unix)]
    const IDLE: &str = "[{\"id\":\"s1\",\"projectId\":\"p1\",\"status\":\"idle\",\"needsInput\":false},{\"id\":\"s2\",\"projectId\":\"p1\",\"status\":\"interrupted\",\"needsInput\":false}]";
    #[cfg(unix)]
    const RUNNING: &str =
        "[{\"id\":\"s1\",\"projectId\":\"p1\",\"status\":\"running\",\"needsInput\":false}]";
    /// An old Host installed, a package staged for it, and the Host's own API up.
    #[cfg(unix)]
    fn staged_host(sessions: &str) -> (tempfile::TempDir, FakeApi) {
        let api = FakeApi::start(sessions);
        let (ok, stderr, _, home) = run_bootstrap_on(
            true,
            None,
            false,
            Some("0.9.4"),
            upgrade_step_on(api.port, UpgradePhase::Stage),
        );
        assert!(ok, "{stderr}");
        (home, api)
    }
    /// Runs the swap worker the way the launcher would (detached: the output is its own log).
    #[cfg(unix)]
    fn run_worker(home: &std::path::Path, api: &FakeApi, close_output: bool) -> Option<i32> {
        run_in_home_env(
            home,
            &upgrade_step_on(api.port, UpgradePhase::Swap),
            &api.envs(),
            close_output,
        )
        .0
    }

    #[cfg(unix)]
    #[test]
    fn staging_downloads_and_verifies_without_touching_the_running_host() {
        for installed in [
            "1.0.0+monoptah",
            "0.9.4",
            "9.0.0",
            "1.1.0-rc.1+monoptah",
            "",
        ] {
            let (ok, stderr, urls, home) = run_bootstrap_on(
                true,
                None,
                false,
                Some(installed),
                upgrade_step(UpgradePhase::Stage),
            );
            assert!(ok, "{installed}: {stderr}");
            assert_eq!(urls.len(), 2, "{installed}: {urls:?}");
            let base = home.path().join(".monocode-host");
            // The old runtime and launcher are exactly as they were, and the Host was not stopped.
            assert!(
                runtime_path(home.path()).ends_with("runtime/old"),
                "{installed}"
            );
            assert!(!base.join("runtime-path.prev").exists());
            let calls = host_calls(&home);
            assert!(!calls.contains("service"), "{installed}: {calls}");
            // A verified package waits in a staging directory.
            let staged = std::fs::read_to_string(base.join("staged-runtime")).unwrap();
            assert!(std::path::Path::new(staged.trim())
                .join("monocode-host")
                .is_file());
        }
        for installed in [super::super::host_release::HOST_VERSION, "99.0.0+monoptah"] {
            let (ok, stderr, urls, home) = run_bootstrap_on(
                true,
                None,
                false,
                Some(installed),
                upgrade_step(UpgradePhase::Stage),
            );
            assert!(ok, "{installed}: {stderr}");
            assert!(urls.is_empty(), "{installed}: {urls:?}");
            assert!(!home.path().join(".monocode-host/staged-runtime").exists());
        }
        // Without the flag an installed Host is never replaced: the plain connect rule.
        let (ok, _, urls, home) = run_bootstrap_on(
            true,
            None,
            false,
            Some("0.9.4"),
            bootstrap_script(HostPlatform::Unix),
        );
        assert!(ok);
        assert!(urls.is_empty());
        assert!(!host_calls(&home).contains("service uninstall"));
    }
    #[cfg(unix)]
    #[test]
    fn a_failed_stage_leaves_the_old_host_and_launcher_alone() {
        for (pin, publish, tamper, message) in [
            (false, None, false, "checksum mismatch"),
            (true, None, true, "package checksum mismatch"),
            (
                true,
                Some("0".repeat(64)),
                false,
                "pinned checksum mismatch",
            ),
        ] {
            let (ok, stderr, _, home) = run_bootstrap_on(
                pin,
                publish.as_deref(),
                tamper,
                Some("0.9.4"),
                upgrade_step(UpgradePhase::Stage),
            );
            assert!(!ok);
            assert!(stderr.contains(message), "{stderr}");
            assert!(!host_calls(&home).contains("service"));
            assert!(runtime_path(home.path()).ends_with("runtime/old"));
            assert!(!home.path().join(".monocode-host/staged-runtime").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_swap_installs_the_staged_package_and_keeps_the_previous_host_as_prev() {
        let (home, api) = staged_host(IDLE);
        assert_eq!(run_worker(home.path(), &api, false), Some(0));
        let base = home.path().join(".monocode-host");
        assert_eq!(swap_status(home.path()), "done");
        assert!(std::fs::read_to_string(base.join("swap-info"))
            .unwrap()
            .contains("3774"));
        assert!(!runtime_path(home.path()).ends_with("runtime/old"));
        assert!(std::fs::read_to_string(base.join("runtime-path.prev"))
            .unwrap()
            .trim()
            .ends_with("runtime/old"));
        assert!(base.join("bin/monocode-host.prev").is_file());
        assert!(!base.join("staged-runtime").exists());
        assert!(host_calls(&home).contains("new service install"));
        // A swap with nothing staged is refused and changes nothing.
        assert_eq!(run_worker(home.path(), &api, false), Some(3));
        assert_eq!(swap_status(home.path()), "refused 3");
    }
    #[cfg(unix)]
    #[test]
    fn the_last_look_on_the_host_itself_stops_the_swap_before_anything_is_touched() {
        for sessions in [
            RUNNING,
            "[{\"id\":\"s1\",\"projectId\":\"p1\",\"status\":\"idle\",\"needsInput\":true}]",
            // A session without a readable status counts as busy.
            "[{\"id\":\"s1\",\"projectId\":\"p1\"}]",
        ] {
            let (home, api) = staged_host(sessions);
            let before = host_calls(&home);
            assert_eq!(run_worker(home.path(), &api, false), Some(77), "{sessions}");
            assert_eq!(swap_status(home.path()), "busy");
            // The Host is untouched: same runtime, never stopped, and the package stays.
            assert!(runtime_path(home.path()).ends_with("runtime/old"));
            let calls = host_calls(&home);
            assert!(!calls.contains("service uninstall"), "{calls}");
            assert_eq!(
                calls.matches("service").count(),
                before.matches("service").count()
            );
            assert!(home.path().join(".monocode-host/staged-runtime").exists());
        }
        // The same staged package goes in once the Host is idle.
        let (home, api) = staged_host(RUNNING);
        assert_eq!(run_worker(home.path(), &api, false), Some(77));
        api.set(IDLE);
        assert_eq!(run_worker(home.path(), &api, false), Some(0));
        assert_eq!(swap_status(home.path()), "done");
        // A Host that cannot be asked (wrong credential, nothing listening) is busy too.
        let (home, api) = staged_host(IDLE);
        let wrong = [
            ("MONOCODE_CHECK_TOKEN", "wrong"),
            ("MONOCODE_CHECK_ENV", "env-1"),
        ];
        let code = run_in_home_env(
            home.path(),
            &upgrade_step_on(api.port, UpgradePhase::Swap),
            &wrong,
            false,
        )
        .0;
        assert_eq!(code, Some(77));
        let code = run_in_home_env(
            home.path(),
            &upgrade_step_on(api.port, UpgradePhase::Swap),
            &[],
            false,
        )
        .0;
        assert_eq!(
            code,
            Some(77),
            "no credential: cannot look, so it does not swap"
        );
        assert!(runtime_path(home.path()).ends_with("runtime/old"));
    }
    #[cfg(unix)]
    #[test]
    fn a_swap_without_a_way_back_is_refused_and_the_host_is_left_alone() {
        let (home, api) = staged_host(IDLE);
        // The launcher is there but nothing says which runtime it runs: no rollback possible.
        let base = home.path().join(".monocode-host");
        std::fs::remove_file(base.join("runtime-path")).unwrap();
        assert_eq!(run_worker(home.path(), &api, false), Some(78));
        assert_eq!(swap_status(home.path()), "refused 78");
        assert!(base.join("bin/monocode-host").is_file());
        assert!(!host_calls(&home).contains("service uninstall"));
        assert!(base.join("staged-runtime").exists());
    }
    #[cfg(unix)]
    #[test]
    fn a_new_host_that_fails_to_start_is_rolled_back_to_the_old_one() {
        let (home, api) = staged_host(IDLE);
        // The new Host's `service install` fails.
        std::fs::write(home.path().join("fail-new"), "").unwrap();
        assert_eq!(run_worker(home.path(), &api, false), Some(75));
        assert_eq!(swap_status(home.path()), "restored");
        // The old runtime and launcher are back and the old Host was started again.
        assert!(runtime_path(home.path()).ends_with("runtime/old"));
        assert!(host_calls(&home).contains("old service install"));
        let launcher = home.path().join(".monocode-host/bin/monocode-host");
        let output = Command::new(launcher)
            .arg("connection-info")
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("3774"));
    }
    #[cfg(unix)]
    #[test]
    fn a_rollback_that_cannot_restart_the_old_host_ends_unrestored() {
        let (home, api) = staged_host(IDLE);
        std::fs::write(home.path().join("fail-new"), "").unwrap();
        // The old Host does not start again either.
        std::fs::write(home.path().join("fail-old"), "").unwrap();
        assert_eq!(run_worker(home.path(), &api, false), Some(76));
        assert_eq!(swap_status(home.path()), "unrestored");
    }
    #[cfg(unix)]
    #[test]
    fn a_closed_ssh_neither_stops_the_swap_nor_turns_a_good_update_into_a_rollback() {
        // stdout and stderr go nowhere (a broken pipe), including the final connection-info.
        let (home, api) = staged_host(IDLE);
        assert_eq!(run_worker(home.path(), &api, true), Some(0));
        assert_eq!(swap_status(home.path()), "done");
        assert!(
            !runtime_path(home.path()).ends_with("runtime/old"),
            "not rolled back"
        );
        // And a failing update with the ssh gone still rolls back.
        let (home, api) = staged_host(IDLE);
        std::fs::write(home.path().join("fail-new"), "").unwrap();
        assert_eq!(run_worker(home.path(), &api, true), Some(75));
        assert_eq!(swap_status(home.path()), "restored");
        assert!(runtime_path(home.path()).ends_with("runtime/old"));
    }
    #[cfg(unix)]
    #[test]
    fn the_swap_installs_nothing_slow_after_the_host_is_stopped() {
        use std::os::unix::fs::PermissionsExt;
        // A `curl` that records a Claude install and forwards everything else.
        let stubs = tempfile::tempdir().unwrap();
        let curl = stubs.path().join("curl");
        std::fs::write(
            &curl,
            "#!/bin/sh\ncase \"$*\" in *claude.ai*) echo called >> \"$HOME/claude-install.log\"; exit 0 ;; esac\nfor c in /usr/bin/curl /opt/homebrew/bin/curl /usr/local/bin/curl; do [ -x \"$c\" ] && exec \"$c\" \"$@\"; done\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:/usr/bin:/bin", stubs.path().display());
        let claude_flags = "MONOCODE_HOST_TRACK_TOKEN=1\nMONOCODE_HOST_INSTALL_CLAUDE=1\n";
        // Control: a plain connect bootstrap does install Claude when it is missing.
        let (ok, stderr, _, home) = run_bootstrap_on(
            true,
            None,
            false,
            Some(super::super::host_release::HOST_VERSION),
            format!("{claude_flags}{}", bootstrap_script(HostPlatform::Unix)),
        );
        let _ = (ok, stderr, home);
        let control = tempfile::tempdir().unwrap();
        install_old_host(control.path(), super::super::host_release::HOST_VERSION);
        let (code, _, stderr) = run_in_home_env(
            control.path(),
            &format!("{claude_flags}{}", bootstrap_script(HostPlatform::Unix)),
            &[("PATH", path.as_str())],
            false,
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert!(
            control.path().join("claude-install.log").exists(),
            "the stub sees installs"
        );
        // The swap worker skips it, however the script asked.
        let (home, api) = staged_host(IDLE);
        let mut envs = api.envs();
        envs.push(("PATH", path.as_str()));
        let script = format!(
            "{claude_flags}{}",
            upgrade_step_on(api.port, UpgradePhase::Swap)
        );
        let (code, _, _) = run_in_home_env(home.path(), &script, &envs, false);
        assert_eq!(code, Some(0));
        assert_eq!(swap_status(home.path()), "done");
        assert!(!home.path().join("claude-install.log").exists());
    }
    /// A swap whose worker died: a package staged, the previous Host's files saved, and
    /// `runtime-path` pointing at `current` (`"staged"` for the new Host, `"old"` for the
    /// untouched one).
    #[cfg(unix)]
    fn died_mid_swap(current: &str) -> tempfile::TempDir {
        let (home, _api) = staged_host(IDLE);
        let base = home.path().join(".monocode-host");
        std::fs::copy(base.join("runtime-path"), base.join("runtime-path.prev")).unwrap();
        std::fs::copy(
            base.join("bin/monocode-host"),
            base.join("bin/monocode-host.prev"),
        )
        .unwrap();
        if current == "staged" {
            let staged = std::fs::read_to_string(base.join("staged-runtime")).unwrap();
            std::fs::write(base.join("runtime-path"), staged).unwrap();
        }
        std::fs::write(base.join("swap-status"), "swapping 999999\nnonce-1\n").unwrap();
        home
    }
    #[cfg(unix)]
    fn status_of(home: &std::path::Path, nonce: &str) -> String {
        run_in_home_env(
            home,
            &status_script(nonce),
            &[("MONOCODE_PROBE_WAIT", "0")],
            false,
        )
        .1
        .trim()
        .to_string()
    }
    #[cfg(unix)]
    #[test]
    fn status_reports_a_live_worker_and_one_that_never_got_to_touch_anything() {
        let (home, _api) = staged_host(IDLE);
        let base = home.path().join(".monocode-host");
        assert_eq!(status_of(home.path(), "nonce-1"), "{\"swap\":\"none\"}");
        // A live worker is left alone: its command line is the worker's (Linux), or its
        // heartbeat is fresh (elsewhere).
        let mut worker = fake_worker();
        std::fs::write(base.join("swap-heartbeat"), "").unwrap();
        std::fs::write(
            base.join("swap-status"),
            format!("swapping {}\nnonce-1\n", worker.id()),
        )
        .unwrap();
        assert_eq!(status_of(home.path(), "nonce-1"), "{\"swap\":\"swapping\"}");
        let _ = worker.wait();
        // A worker that died before it touched anything.
        std::fs::write(base.join("swap-status"), "checking 999999\nnonce-1\n").unwrap();
        assert_eq!(status_of(home.path(), "nonce-1"), "{\"swap\":\"failed\"}");
    }
    #[cfg(unix)]
    #[test]
    fn a_reused_pid_is_not_taken_for_the_worker() {
        let home = died_mid_swap("old");
        let base = home.path().join(".monocode-host");
        // The pid belongs to a live process now, but it is not the worker: not its command
        // line, and no heartbeat.
        let _ = std::fs::remove_file(base.join("swap-heartbeat"));
        std::fs::write(
            base.join("swap-status"),
            format!("swapping {}\nnonce-1\n", std::process::id()),
        )
        .unwrap();
        assert_eq!(status_of(home.path(), "nonce-1"), "{\"swap\":\"restored\"}");
        // A stale heartbeat is no proof either (no /proc: macOS), and an old file does not
        // keep a dead worker alive forever.
        let mut other = Command::new("sleep").arg("3").spawn().unwrap();
        std::fs::write(
            base.join("swap-status"),
            format!("swapping {}\nnonce-1\n", other.id()),
        )
        .unwrap();
        assert_eq!(status_of(home.path(), "nonce-1"), "{\"swap\":\"restored\"}");
        let _ = other.wait();
    }
    #[cfg(unix)]
    #[test]
    fn recovery_never_restarts_a_host_that_answers() {
        // (a) The worker died with the old files in place: the old Host is fine, nothing to undo.
        let home = died_mid_swap("old");
        assert_eq!(status_of(home.path(), "nonce-1"), "{\"swap\":\"restored\"}");
        assert_eq!(swap_status(home.path()), "restored");
        let calls = host_calls(&home);
        assert!(
            !calls.contains("service"),
            "the healthy Host was restarted: {calls}"
        );
        assert!(runtime_path(home.path()).ends_with("runtime/old"));
        // (b) The worker died after the swap and one health probe fails by chance: the new
        // Host is kept, not rolled back over.
        let home = died_mid_swap("staged");
        std::fs::write(home.path().join("flaky"), "2").unwrap();
        assert_eq!(
            status_of(home.path(), "nonce-1"),
            "{\"swap\":\"done\",\"port\":3774}"
        );
        let calls = host_calls(&home);
        assert!(
            !calls.contains("service"),
            "the healthy Host was restarted: {calls}"
        );
        assert!(!runtime_path(home.path()).ends_with("runtime/old"));
        // Only when nothing answers (more failures than the probe tries) is the old Host put back.
        let home = died_mid_swap("staged");
        std::fs::write(home.path().join("flaky"), "7").unwrap();
        assert_eq!(status_of(home.path(), "nonce-1"), "{\"swap\":\"restored\"}");
        assert!(runtime_path(home.path()).ends_with("runtime/old"));
        assert!(host_calls(&home).contains("old service install"));
    }
    #[cfg(unix)]
    #[test]
    fn a_status_from_another_swap_is_not_this_ones() {
        let home = died_mid_swap("old");
        assert_eq!(status_of(home.path(), "other"), "{\"swap\":\"stale\"}");
        // Nothing was settled on the strength of it.
        assert_eq!(swap_status(home.path()), "swapping 999999");
        assert!(!host_calls(&home).contains("service"));
    }
    #[cfg(unix)]
    #[test]
    fn the_credential_never_reaches_a_process_the_worker_starts() {
        let present = |home: &tempfile::TempDir| {
            let log = env_log(home);
            assert!(!log.is_empty(), "the stub Host ran");
            log.contains("MONOCODE_CHECK") || log.contains(API_TOKEN)
        };
        // A good update: the new Host is started.
        let (home, api) = staged_host(IDLE);
        assert_eq!(run_worker(home.path(), &api, false), Some(0));
        assert!(host_calls(&home).contains("new service install"));
        assert!(!present(&home), "{}", env_log(&home));
        // A rollback: the new Host fails, the old one is started again.
        let (home, api) = staged_host(IDLE);
        std::fs::write(home.path().join("fail-new"), "").unwrap();
        assert_eq!(run_worker(home.path(), &api, false), Some(75));
        assert!(host_calls(&home).contains("old service install"));
        assert!(!present(&home), "{}", env_log(&home));
        // A busy Host: exit 77, and the staged check that ran before it saw nothing either.
        let (home, api) = staged_host(RUNNING);
        assert_eq!(run_worker(home.path(), &api, false), Some(77));
        assert!(!present(&home), "{}", env_log(&home));
        // A refusal before the look.
        let (home, api) = staged_host(IDLE);
        std::fs::remove_file(home.path().join(".monocode-host/runtime-path")).unwrap();
        assert_eq!(run_worker(home.path(), &api, false), Some(78));
        assert!(!present(&home), "{}", env_log(&home));
    }
    #[cfg(unix)]
    #[test]
    fn a_running_session_without_a_project_key_is_not_invisible() {
        // Schema drift: the running session has no `projectId`, the idle one does.
        let adversarial = "[{\"revision\":1,\"status\":\"running\",\"id\":\"a\",\"title\":\"t\",\"needsInput\":false},{\"projectId\":\"p1\",\"status\":\"idle\",\"id\":\"b\",\"needsInput\":false}]";
        let (home, api) = staged_host(adversarial);
        assert_eq!(run_worker(home.path(), &api, false), Some(77));
        assert!(runtime_path(home.path()).ends_with("runtime/old"));
        // A session with no status at all, and one with an unknown status, fail closed too.
        for sessions in [
            "[{\"projectId\":\"p1\",\"id\":\"a\"},{\"projectId\":\"p1\",\"status\":\"idle\"}]",
            "[{\"projectId\":\"p1\",\"status\":\"mystery\"}]",
        ] {
            let (home, api) = staged_host(sessions);
            assert_eq!(run_worker(home.path(), &api, false), Some(77), "{sessions}");
        }
    }
    #[cfg(unix)]
    #[test]
    fn a_second_launcher_cannot_start_while_a_worker_is_alive() {
        let (home, api) = staged_host(IDLE);
        let base = home.path().join(".monocode-host");
        let check = SwapCheck {
            device_token: API_TOKEN,
            environment_id: "env-1",
            nonce: "nonce-2",
        };
        let launcher = codespace_upgrade_script(api.port, UpgradePhase::Swap, Some(check));
        // A live worker owns the lock.
        let mut worker = fake_worker();
        std::fs::write(base.join("swap.lock"), format!("{}\n", worker.id())).unwrap();
        std::fs::write(base.join("swap-heartbeat"), "").unwrap();
        std::fs::write(base.join("swap-status"), "swapping 1\nnonce-1\n").unwrap();
        let (code, _, stderr) = run_in_home(home.path(), &launcher);
        assert_eq!(code, Some(1));
        assert!(stderr.contains("still running"), "{stderr}");
        // Nothing of the first swap was disturbed.
        assert_eq!(status_nonce(home.path()), "nonce-1");
        assert!(!base.join("swap-worker.sh").exists());
        let _ = worker.wait();
        // A lock left by a dead worker does not block.
        std::fs::write(base.join("swap.lock"), "999999\n").unwrap();
        let (code, stdout, stderr) = run_in_home(home.path(), &launcher);
        assert_eq!(code, Some(0), "{stderr}");
        assert!(stdout.contains("started"));
        for _ in 0..150 {
            if swap_status(home.path()) == "done" {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        assert_eq!(swap_status(home.path()), "done");
    }
    #[cfg(unix)]
    #[test]
    fn the_launcher_runs_the_worker_detached_and_keeps_every_credential_off_the_disk() {
        let (home, api) = staged_host(IDLE);
        let base = home.path().join(".monocode-host");
        let check = SwapCheck {
            device_token: API_TOKEN,
            environment_id: "env-1",
            nonce: "nonce-1",
        };
        let launcher = codespace_upgrade_script(api.port, UpgradePhase::Swap, Some(check));
        let (code, stdout, stderr) = run_in_home(home.path(), &launcher);
        assert_eq!(code, Some(0), "{stderr}");
        assert!(stdout.contains("{\"swap\":\"started\"}"), "{stdout}");
        // The launcher's own shell no longer holds the credential once the worker runs.
        // The launcher is gone; the worker finishes on its own.
        let mut done = false;
        for _ in 0..150 {
            if swap_status(home.path()) == "done" {
                done = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(done, "status: {}", swap_status(home.path()));
        assert_eq!(status_nonce(home.path()), "nonce-1");
        assert!(!env_log(&home).contains(API_TOKEN) && !env_log(&home).contains("MONOCODE_CHECK"));
        assert!(!runtime_path(home.path()).ends_with("runtime/old"));
        let worker = std::fs::read_to_string(base.join("swap-worker.sh")).unwrap();
        let log = std::fs::read_to_string(base.join("swap.log")).unwrap_or_default();
        for text in [&worker, &log] {
            assert!(!text.contains(API_TOKEN), "a credential reached the disk");
        }
        assert!(worker.contains("MONOCODE_HOST_UPGRADE_PHASE=swapnow"));
    }

    #[cfg(unix)]
    #[test]
    fn discard_drops_a_staged_package() {
        let (ok, stderr, _, home) = run_bootstrap_on(
            true,
            None,
            false,
            Some("0.9.4"),
            upgrade_step(UpgradePhase::Stage),
        );
        assert!(ok, "{stderr}");
        let base = home.path().join(".monocode-host");
        let staged = std::fs::read_to_string(base.join("staged-runtime")).unwrap();
        let (code, stderr, _) = run_in_home(home.path(), &upgrade_step(UpgradePhase::Discard));
        assert_eq!(code, Some(0), "{stderr}");
        assert!(!std::path::Path::new(staged.trim()).exists());
        assert!(!base.join("staged-runtime").exists());
        assert!(runtime_path(home.path()).ends_with("runtime/old"));
    }
    #[test]
    fn cloud_upgrade_scripts_carry_the_flags_and_the_same_prelude_as_connect() {
        let prelude = "unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\nexport CLAUDE_CODE_OAUTH_TOKEN='sk-test'\n";
        let check = SwapCheck {
            device_token: "device-token-xyz",
            environment_id: "env-9",
            nonce: "nonce-9",
        };
        for phase in [
            UpgradePhase::Stage,
            UpgradePhase::Discard,
            UpgradePhase::Status,
        ] {
            let flags = format!(
                "MONOCODE_HOST_UPGRADE_IF_OLDER=1\nMONOCODE_HOST_UPGRADE_PHASE={}\nMONOCODE_HOST_PORT=3790\n",
                phase.word()
            );
            let sandbox = sandbox_upgrade_script(Some(prelude), 3790, phase, None);
            assert!(sandbox.starts_with(prelude), "{sandbox}");
            // Same body as the connect step, with the same detached start.
            let connect = sandbox_bootstrap_script(Some(prelude));
            assert!(
                sandbox.replace(&flags, "") == connect,
                "only the flags differ"
            );
            assert!(sandbox.contains("ensure_detached_host"));
            assert!(!sandbox_upgrade_script(None, 3774, phase, None).contains("sk-test"));
            assert!(!sandbox.contains("device-token-xyz"));
            let codespace = codespace_upgrade_script(3774, phase, None);
            assert!(codespace.starts_with(&format!(
                "MONOCODE_HOST_UPGRADE_IF_OLDER=1\nMONOCODE_HOST_UPGRADE_PHASE={}\nMONOCODE_HOST_PORT=3774\n",
                phase.word()
            )));
            assert!(codespace.ends_with(&codespace_bootstrap_script()));
        }
        // The swap is a launcher: the prelude and the credential exports come first, then the
        // worker (with the same body) goes into a file and runs detached.
        let swap = sandbox_upgrade_script(Some(prelude), 3790, UpgradePhase::Swap, Some(check));
        assert!(swap.starts_with(prelude), "{swap}");
        assert!(swap.contains("export MONOCODE_CHECK_TOKEN='device-token-xyz'\n"));
        assert!(swap.contains("export MONOCODE_CHECK_ENV='env-9'\n"));
        assert!(swap.contains(
            "MONOCODE_HOST_UPGRADE_PHASE=swapnow\nMONOCODE_HOST_PORT=3790\nMONOCODE_SWAP_NONCE='nonce-9'\n"
        ));
        // The status script asks for the same swap.
        assert!(
            codespace_upgrade_script(3774, UpgradePhase::Status, Some(check))
                .contains("MONOCODE_SWAP_NONCE='nonce-9'\n")
        );
        // The launcher refuses a second worker and drops the credential before it returns.
        assert!(
            swap.contains("swap.lock")
                && swap.contains("unset MONOCODE_CHECK_TOKEN MONOCODE_CHECK_ENV")
        );
        assert!(swap.contains("ensure_detached_host"));
        assert!(swap.contains("setsid nohup sh \"$BASE/swap-worker.sh\""));
        // The prelude's secret is in the launcher only, not in the file the worker is saved to.
        let worker = swap.split("<<'MONOCODE_SWAP_WORKER_END'\n").nth(1).unwrap();
        let worker = worker.split("\nMONOCODE_SWAP_WORKER_END\n").next().unwrap();
        assert!(!worker.contains("sk-test") && !worker.contains("device-token-xyz"));
        let codespace = codespace_upgrade_script(3774, UpgradePhase::Swap, Some(check));
        assert!(codespace.contains("export MONOCODE_CHECK_TOKEN='device-token-xyz'\n"));
        // The user's explicit Upgrade is untouched: it forces the install.
        assert!(
            upgrade_script(HostPlatform::Unix, 3774).starts_with("MONOCODE_HOST_FORCE_UPGRADE=1\n")
        );
    }
    #[cfg(unix)]
    #[test]
    fn is_older_or_foreign_compares_the_semver_core_and_the_flavor() {
        let template = include_str!("remote_bootstrap.sh");
        let start = template.find("is_older_or_foreign() {").unwrap();
        let end = start + template[start..].find("\n}\n").unwrap() + 3;
        let function = &template[start..end];
        let check = |installed: &str, pin: &str| {
            Command::new("sh")
                .arg("-c")
                .arg(format!("{function}\nis_older_or_foreign \"$1\" \"$2\""))
                .arg("sh")
                .arg(installed)
                .arg(pin)
                .status()
                .unwrap()
                .success()
        };
        let pin = "1.1.0+monoptah";
        assert!(check("1.1.0", pin), "upstream, same number");
        assert!(check("2.0.0", pin), "upstream, higher number");
        assert!(check("1.0.9+monoptah", pin));
        assert!(check("0.33.0+monoptah", pin));
        assert!(check("1.0.10+monoptah", "1.1.0+monoptah"));
        assert!(check("garbage+monoptah", pin));
        assert!(!check("1.1.0+monoptah", pin));
        assert!(!check("1.1.1+monoptah", pin));
        assert!(!check("1.2.0+monoptah", pin));
        assert!(!check("10.0.0+monoptah", pin));
        // Pre-releases are older than their release, build metadata is ignored.
        assert!(check("1.1.0-rc.1+monoptah", pin));
        assert!(check("1.1.0-rc.1+monoptah.3", pin));
        assert!(!check("1.1.0+monoptah.3", pin));
        assert!(!check("1.1.0+monoptah.3", "1.1.0+monoptah"));
        assert!(check("1.1.0+monoptah.3", "1.1.1+monoptah"));
        assert!(
            !check("1.1.1-rc.1+monoptah", pin),
            "a pre-release of a higher core"
        );
        assert!(!check("1.1.0+monoptah", "1.1.0-rc.1+monoptah"));
        assert!(!check("1.1.0-rc.1+monoptah", "1.1.0-rc.1+monoptah"));
        assert!(check("1.1.0-rc.1+monoptah", "1.1.0-rc.2+monoptah"));
        assert!(check("1.1.0+monoptahx", pin), "not the flavor marker");
        assert!(check("1.1.0+other", pin));
    }
    #[test]
    fn remote_platform_probe_handles_cmd_powershell_and_unix() {
        assert_eq!(
            parse_platform("MONOCODE_PLATFORM $env:OS Windows_NT $OS\r\n").unwrap(),
            HostPlatform::Windows
        );
        assert_eq!(
            parse_platform("MONOCODE_PLATFORM\r\nWindows_NT\r\n%OS%\r\n").unwrap(),
            HostPlatform::Windows
        );
        assert_eq!(
            parse_platform("MONOCODE_PLATFORM :OS %OS%\n").unwrap(),
            HostPlatform::Unix
        );
        assert!(parse_platform("unrecognized shell").is_err());
        assert!(powershell_reader().len() < 4096);
        let script = bootstrap_script(HostPlatform::Windows);
        assert!(!script.contains("@@"));
        assert!(script.contains("checksum mismatch"));
        assert!(script.contains("Protect-MonoCodeDirectory"));
        assert!(pairing_script(HostPlatform::Windows, "Nick's $PC").contains("'Nick''s $PC'"));
    }
    #[cfg(windows)]
    #[test]
    fn windows_shells_detect_the_platform_and_accept_utf8_scripts() {
        for (program, args) in [
            ("cmd.exe", vec!["/D", "/C"]),
            (
                "powershell.exe",
                vec!["-NoProfile", "-NonInteractive", "-Command"],
            ),
        ] {
            let output = Command::new(program)
                .env_remove("PSModulePath")
                .args(args)
                .arg(PLATFORM_PROBE.join(" "))
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                parse_platform(&String::from_utf8_lossy(&output.stdout)).unwrap(),
                HostPlatform::Windows
            );
        }
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &powershell_reader(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all("Write-Output '日本語 🖥'\n".as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "日本語 🖥");
    }
    #[test]
    fn device_names_are_bounded_single_lines() {
        let name = device_name();
        assert!(name.starts_with("Monoptah"));
        assert!(name.chars().count() <= 100);
        assert!(!name.chars().any(char::is_control));
    }
    #[test]
    fn answers_must_match_the_current_prompt() {
        let job = Job::new();
        assert!(job.answer("old", "yes".into()).is_err());
        let waiter = job.clone();
        let thread = std::thread::spawn(move || waiter.prompt("Trust this host?".into(), true));
        while job.view().prompt.is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let prompt = job.view().prompt.unwrap();
        assert!(job.answer(&prompt.id, "arbitrary".into()).is_err());
        job.answer(&prompt.id, "yes".into()).unwrap();
        assert_eq!(thread.join().unwrap(), Some("yes".into()));
        assert!(job.answer(&prompt.id, "yes".into()).is_err());
    }
    fn sandbox_target() -> SshTarget {
        SshTarget {
            target: "sbx_001abc@sbx_cloud".into(),
            port: None,
            remote_port: 3774,
            codespace: None,
            sandbox: Some(SandboxTarget {
                id: "sbx_001abc".into(),
                name: "my-box".into(),
                workspace: "/home/agent/ws".into(),
                relay_port: 47200,
                config: None,
                mode: SandboxMode::Container,
                docker_account: None,
            }),
        }
    }
    #[test]
    fn sandbox_commands_are_plain_ssh_to_the_alias_without_prompts() {
        let command = command_with_program(&sandbox_target(), true, "ssh".as_ref()).unwrap();
        assert_eq!(command.get_program(), "ssh");
        let args = argv(&command);
        assert_eq!(args[0], "-T");
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"ExitOnForwardFailure=yes".to_string()));
        assert!(!args.contains(&"BatchMode=no".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("StrictHostKeyChecking")));
        assert!(!args
            .iter()
            .any(|a| a.starts_with("NumberOfPasswordPrompts")));
        assert!(!args.contains(&"-p".to_string()));
        assert!(!args.contains(&"codespace".to_string()));
        assert!(args.contains(&"ConnectTimeout=120".to_string()));
        assert!(!args.contains(&"ConnectTimeout=15".to_string()));
        // The destination is added by the caller after `--`.
        assert!(!args.contains(&"--".to_string()));
    }
    fn env_var(command: &Command, key: &str) -> Option<String> {
        command
            .get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    }
    fn account_target(account: Option<&str>) -> SshTarget {
        let mut target = sandbox_target();
        target.sandbox.as_mut().unwrap().docker_account = account.map(str::to_owned);
        target
    }
    #[test]
    fn a_sandbox_in_a_docker_account_gets_its_token_in_the_environment_only() {
        use crate::docker_accounts::test_support::Installed;
        let installed = Installed::new();
        let id = installed.add("alice");
        let target = account_target(Some(&id));
        for command in [
            command_with_program(&target, true, "ssh".as_ref()).unwrap(),
            command_with_program(&target, false, "ssh".as_ref()).unwrap(),
            // The funnel that run_script, detect_platform and Tunnel::start use.
            command(&target, true).unwrap(),
            command(&target, false).unwrap(),
        ] {
            let token = env_var(&command, "DOCKER_ACCESS_TOKEN").expect("token missing");
            assert!(installed.hub.issued().contains(&token));
            assert!(
                argv(&command).iter().all(|a| !a.contains(&token)),
                "{:?}",
                argv(&command)
            );
            // The rest of the command is the same as without an account.
            assert_eq!(
                argv(&command),
                argv(&command_with_program(&sandbox_target(), true, "ssh".as_ref()).unwrap())
            );
        }
    }
    #[test]
    fn a_sandbox_without_an_account_and_other_transports_get_no_token() {
        use crate::docker_accounts::test_support::Installed;
        let installed = Installed::new();
        installed.add("alice");
        let before = installed.hub.calls();
        for target in [
            account_target(None),
            codespace_target("octo-abc123"),
            SshTarget {
                target: "me@host".into(),
                port: Some(22),
                remote_port: 3774,
                codespace: None,
                sandbox: None,
            },
        ] {
            let program = if target.codespace.is_some() {
                "gh"
            } else {
                "ssh"
            };
            let command = command_with_program(&target, true, program.as_ref()).unwrap();
            assert!(
                command.get_envs().all(|(k, _)| k != "DOCKER_ACCESS_TOKEN"),
                "{target:?}"
            );
        }
        assert_eq!(installed.hub.calls(), before, "no account, no exchange");
    }
    #[test]
    fn a_sandbox_whose_account_has_no_token_fails_instead_of_using_the_machine_login() {
        use crate::docker_accounts::test_support::{Installed, Uninstalled};
        {
            let _none = Uninstalled::new();
            let error = command(&account_target(Some("acct")), true).unwrap_err();
            assert!(error.contains("not available"), "{error}");
        }
        let _installed = Installed::new();
        let error = command(&account_target(Some("removed")), true).unwrap_err();
        assert!(error.contains("no longer exists"), "{error}");
        // The tunnel is not started either.
        let error = Tunnel::start(&account_target(Some("removed")), None, None)
            .err()
            .unwrap();
        assert!(error.contains("no longer exists"), "{error}");
    }
    #[test]
    fn a_new_ssh_after_a_rejection_gets_a_fresh_token() {
        use crate::docker_accounts::test_support::Installed;
        let installed = Installed::new();
        let id = installed.add("alice");
        let target = account_target(Some(&id));
        let first = env_var(&command(&target, false).unwrap(), "DOCKER_ACCESS_TOKEN").unwrap();
        // A live tunnel keeps its environment; a reconnect spawns a new ssh, which reads the
        // cache again. While the JWT is valid that is the same one, after a rejection a new one.
        let again = env_var(&command(&target, false).unwrap(), "DOCKER_ACCESS_TOKEN").unwrap();
        assert_eq!(first, again);
        crate::docker_accounts::shared().unwrap().invalidate(&id);
        let fresh = env_var(&command(&target, false).unwrap(), "DOCKER_ACCESS_TOKEN").unwrap();
        assert_ne!(first, fresh);
    }
    #[test]
    fn the_account_of_a_sandbox_target_round_trips_and_is_absent_when_unset() {
        let with: SandboxTarget = serde_json::from_value(serde_json::json!({
            "id":"sbx_1","name":"b","workspace":"/w","relayPort":1,"dockerAccount":"acct"
        }))
        .unwrap();
        assert_eq!(with.docker_account.as_deref(), Some("acct"));
        assert_eq!(
            serde_json::to_value(&with).unwrap()["dockerAccount"],
            "acct"
        );
        // A record written before the field existed.
        let old: SandboxTarget = serde_json::from_value(serde_json::json!({
            "id":"sbx_1","name":"b","workspace":"/w","relayPort":1
        }))
        .unwrap();
        assert_eq!(old.docker_account, None);
        assert!(serde_json::to_value(&old)
            .unwrap()
            .get("dockerAccount")
            .is_none());
    }
    #[test]
    fn a_target_cannot_be_a_codespace_and_a_sandbox() {
        let mut target = sandbox_target();
        target.codespace = Some("octo-abc123".into());
        assert!(check_target(&target).unwrap_err().contains("both"));
    }
    #[test]
    fn golden_outer_argv_and_path_for_plain_ssh() {
        let target = SshTarget {
            target: "me@box".into(),
            port: Some(2222),
            remote_port: 3774,
            codespace: None,
            sandbox: None,
        };
        let command = command_with_program(&target, true, "ssh".as_ref()).unwrap();
        let expected: Vec<&str> = vec![
            "-T",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ForwardX11=no",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "StrictHostKeyChecking=ask",
            "-o",
            "NumberOfPasswordPrompts=3",
            "-o",
            "ForkAfterAuthentication=no",
            "-o",
            "BatchMode=no",
            "-p",
            "2222",
        ];
        assert_eq!(argv(&command), expected);
        assert_eq!(env_of(&command, "LC_ALL").as_deref(), Some("C"));
        assert_eq!(env_of(&command, "PATH"), None);
    }
    #[test]
    fn golden_codespace_env_sets_the_terminal_path() {
        let command =
            command_with_program(&codespace_target("octo-abc123"), false, "gh".as_ref()).unwrap();
        assert_eq!(env_of(&command, "LC_ALL").as_deref(), Some("C"));
        assert_eq!(
            env_of(&command, "PATH").map(std::path::PathBuf::from),
            Some(std::path::PathBuf::from(crate::scm::terminal_search_path()))
        );
        assert!(argv(&command).contains(&"BatchMode=yes".to_string()));
    }
    #[test]
    fn sandbox_scripts_run_in_the_devcontainer_with_the_script_on_stdin() {
        let job = Job::new();
        let askpass = job.askpass().unwrap();
        let mut command = Command::new("sh");
        // Print argv (one per line), then stdin.
        command
            .args(["-c", "for a in \"$@\"; do echo \"ARG:$a\"; done; cat", "sh"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_remote_command(
            &sandbox_target(),
            "echo secret-script\n".into(),
            &job,
            &askpass,
            command,
            &["sh", "-l", "-s"],
        )
        .unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "ARG:--");
        assert_eq!(lines[1], "ARG:sbx_001abc@sbx_cloud");
        assert_eq!(
            lines[2],
            "ARG:$HOME/.devcontainers/bin/devcontainer exec --workspace-folder '/home/agent/ws' -- sh -l -s"
        );
        assert_eq!(lines[3], "echo secret-script");
        assert!(!lines[..3].iter().any(|l| l.contains("secret-script")));
    }
    /// A fake `ssh` that fails with `stderr_text`, as the ProxyCommand's `sbx` does.
    #[cfg(unix)]
    fn failing_ssh(dir: &std::path::Path, stderr_text: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("fake-ssh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho '{stderr_text}' >&2\nexit 255\n"),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }
    #[cfg(unix)]
    #[test]
    fn a_rejected_token_in_a_failed_ssh_script_drops_the_cached_token() {
        use crate::docker_accounts::test_support::Installed;
        let installed = Installed::new();
        let id = installed.add("alice");
        let target = account_target(Some(&id));
        let dir = tempfile::tempdir().unwrap();
        let job = Job::new();
        let askpass = job.askpass().unwrap();
        let token_of =
            || env_var(&command(&target, false).unwrap(), "DOCKER_ACCESS_TOKEN").unwrap();
        // Another failure does not drop it.
        let other = failing_ssh(dir.path(), "ssh: connect to host: Connection refused");
        let first = token_of();
        let error = run_script_with_command(
            &target,
            "true\n".into(),
            &job,
            &askpass,
            command_with_program(&target, true, other.as_os_str()).unwrap(),
        )
        .unwrap_err();
        assert!(error.starts_with("SSH setup failed"), "{error}");
        assert_eq!(token_of(), first, "same JWT: nothing was invalidated");
        // The ProxyCommand's sbx rejected the JWT: the next spawn gets a fresh one.
        let rejecting = failing_ssh(dir.path(), "Error: DOCKER_ACCESS_TOKEN was rejected");
        run_script_with_command(
            &target,
            "true\n".into(),
            &job,
            &askpass,
            command_with_program(&target, true, rejecting.as_os_str()).unwrap(),
        )
        .unwrap_err();
        let fresh = token_of();
        assert_ne!(fresh, first);
        // The JWT is never in the error text.
        let error = run_script_with_command(
            &target,
            "true\n".into(),
            &job,
            &askpass,
            command_with_program(&target, true, rejecting.as_os_str()).unwrap(),
        )
        .unwrap_err();
        for jwt in installed.hub.issued() {
            assert!(!error.contains(&jwt));
        }
    }
    #[cfg(unix)]
    #[test]
    fn a_rejected_token_in_a_failed_tunnel_drops_the_cached_token() {
        use crate::docker_accounts::test_support::Installed;
        let installed = Installed::new();
        let id = installed.add("alice");
        let target = account_target(Some(&id));
        let dir = tempfile::tempdir().unwrap();
        let token_of =
            || env_var(&command(&target, false).unwrap(), "DOCKER_ACCESS_TOKEN").unwrap();
        let first = token_of();
        let rejecting = failing_ssh(dir.path(), "Error: DOCKER_ACCESS_TOKEN was rejected");
        let command = command_with_program(&target, false, rejecting.as_os_str()).unwrap();
        let error = Tunnel::start_with_command(&target, None, None, command)
            .err()
            .expect("the fake ssh exits, the tunnel must fail");
        assert!(error.starts_with("SSH connection failed"), "{error}");
        assert_ne!(token_of(), first, "a reconnect exchanges a fresh JWT");
        // A sandbox without an account never touches the accounts.
        let before = installed.hub.calls();
        let plain = account_target(None);
        let command = command_with_program(&plain, false, rejecting.as_os_str()).unwrap();
        Tunnel::start_with_command(&plain, None, None, command)
            .err()
            .unwrap();
        assert_eq!(installed.hub.calls(), before);
    }
    // The fake ssh is a shell script.
    #[cfg(unix)]
    #[test]
    fn sandbox_tunnel_forwards_to_the_relay_port_of_the_outer_sandbox() {
        let fixture = tempfile::tempdir().unwrap();
        let log = fixture.path().join("argv");
        let script = fixture.path().join("fake-ssh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" > {log:?}\nexit 1\n",
                log = log.display().to_string()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let command = command_with_program(&sandbox_target(), false, script.as_os_str()).unwrap();
        // The fake exits at once, so the tunnel reports a failed connection.
        let error = match Tunnel::start_with_command(&sandbox_target(), None, None, command) {
            Ok(_) => panic!("fake ssh exits, tunnel must fail"),
            Err(error) => error,
        };
        assert!(error.starts_with("SSH connection failed"), "{error}");
        let args = std::fs::read_to_string(&log).unwrap();
        assert!(
            args.contains(" -N -L 127.0.0.1:")
                && args.contains(":127.0.0.1:47200 -- sbx_001abc@sbx_cloud"),
            "{args}"
        );
        assert!(!args.contains("3774"), "{args}");
    }
    #[cfg(unix)]
    #[test]
    fn plain_sandbox_tunnel_goes_straight_to_the_host_port() {
        let fixture = tempfile::tempdir().unwrap();
        let log = fixture.path().join("argv");
        let script = fixture.path().join("fake-ssh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" > {:?}\nexit 1\n",
                log.display().to_string()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut target = sandbox_target();
        target.remote_port = 4111;
        target.sandbox.as_mut().unwrap().mode = SandboxMode::Plain;
        let command = command_with_program(&target, false, script.as_os_str()).unwrap();
        assert!(Tunnel::start_with_command(&target, None, None, command).is_err());
        let args = std::fs::read_to_string(&log).unwrap();
        assert!(
            args.contains(":127.0.0.1:4111 -- sbx_001abc@sbx_cloud"),
            "{args}"
        );
        assert!(!args.contains("47200"), "{args}");
    }
    #[test]
    fn plain_sandbox_scripts_run_in_the_outer_workspace_without_devcontainer() {
        let mut target = sandbox_target().sandbox.unwrap();
        target.mode = SandboxMode::Plain;
        assert_eq!(
            target.exec_in_container(&["sh", "-l", "-s"]),
            "cd '/home/agent/ws' && sh -l -s"
        );
        assert_eq!(
            target.exec_in_container(&["bash", "-l"]),
            "cd '/home/agent/ws' && bash -l"
        );
    }
    #[test]
    fn sandbox_mode_defaults_to_container_and_is_only_written_when_plain() {
        let old: SandboxTarget =
            serde_json::from_str(r#"{"id":"sbx_1","name":"b","workspace":"/w","relayPort":47200}"#)
                .unwrap();
        assert_eq!(old.mode, SandboxMode::Container);
        assert!(!serde_json::to_string(&old).unwrap().contains("mode"));
        let mut plain = old.clone();
        plain.mode = SandboxMode::Plain;
        let text = serde_json::to_string(&plain).unwrap();
        assert!(text.contains(r#""mode":"plain""#), "{text}");
        assert_eq!(serde_json::from_str::<SandboxTarget>(&text).unwrap(), plain);
    }
    #[test]
    fn sandbox_fields_are_validated_before_they_reach_argv() {
        for id in ["sbx_001abc", "sbx_0"] {
            assert!(validate_sandbox_id(id).is_ok(), "{id}");
        }
        for id in [
            "",
            "sbx_",
            "sbx_ABC",
            "sbx-001",
            "sbx_a b",
            "-oProxyCommand=x",
            "sbx_a;b",
            "xsbx_1",
            "sbx_a@b",
        ] {
            assert!(validate_sandbox_id(id).is_err(), "{id}");
        }
        for name in ["my-box", "ab", "a1", &"a".repeat(63)] {
            assert!(validate_sandbox_name(name).is_ok(), "{name}");
        }
        for name in [
            "",
            "a",
            "-x",
            ".x",
            "a.b",
            "a_b",
            "box_1.2",
            "a b",
            "a;b",
            "a/b",
            "$(x)",
            "é1",
            &"a".repeat(64),
        ] {
            assert!(validate_sandbox_name(name).is_err(), "{name}");
        }
        for path in ["/home/agent/ws", "/a", "/work/my-repo_1.2/"] {
            assert!(validate_sandbox_workspace(path).is_ok(), "{path}");
        }
        for path in [
            "",
            "/",
            "relative/ws",
            "/a/../b",
            "/a/./b",
            "//a",
            "/a//b",
            "/a b",
            "/a;b",
            "/a'b",
            "/a$(x)",
            "/a\nb",
            "~/ws",
        ] {
            assert!(validate_sandbox_workspace(path).is_err(), "{path}");
        }
        let mut target = sandbox_target();
        assert!(check_target(&target).is_ok());
        target.sandbox.as_mut().unwrap().relay_port = 0;
        assert!(check_target(&target).is_err());
        target.sandbox.as_mut().unwrap().relay_port = 1;
        target.sandbox.as_mut().unwrap().workspace = "/a;rm -rf /".into();
        assert!(check_target(&target).is_err());
    }
    #[test]
    fn a_codespace_script_with_a_prelude_starts_with_it_and_without_one_is_unchanged() {
        const PRELUDE: &str = "unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\nexport CLAUDE_CODE_OAUTH_TOKEN='sk-ant-oat01-x'\n";
        let plain = format!(
            "MONOCODE_HOST_TRACK_TOKEN=1\nMONOCODE_HOST_INSTALL_CLAUDE=1\n{}",
            bootstrap_script(HostPlatform::Unix)
        );
        // Byte for byte what it was before accounts existed.
        assert_eq!(codespace_bootstrap_script(), plain);
        assert_eq!(codespace_bootstrap_script_with(None), plain);
        let with = codespace_bootstrap_script_with(Some(PRELUDE));
        assert_eq!(with, format!("{PRELUDE}\n{plain}"));
        assert!(with.ends_with(&plain));
        // The upgrade steps: the same prelude before every phase, the launcher included.
        let check = SwapCheck {
            device_token: "d",
            environment_id: "e",
            nonce: "n",
        };
        for phase in [
            UpgradePhase::Stage,
            UpgradePhase::Swap,
            UpgradePhase::Discard,
            UpgradePhase::Status,
        ] {
            let without = codespace_upgrade_script_with(None, 3774, phase, Some(check));
            assert_eq!(
                without,
                codespace_upgrade_script(3774, phase, Some(check)),
                "{phase:?}"
            );
            let with = codespace_upgrade_script_with(Some(PRELUDE), 3774, phase, Some(check));
            assert_eq!(with, format!("{PRELUDE}\n{without}"), "{phase:?}");
            // Only the prelude carries the secret: it is not part of any script without one.
            assert!(!without.contains("sk-ant-oat01-x"), "{phase:?}");
        }
    }

    #[test]
    fn detached_flag_and_helpers_exist_only_in_the_sandbox_bootstrap() {
        let sandbox = sandbox_bootstrap_script(None);
        assert!(sandbox.contains("MONOCODE_HOST_DETACHED=1\n"));
        assert!(sandbox.contains("ensure_detached_host\n\"$ENTRY\" service install"));
        assert!(!sandbox.contains("restart_detached_host"));
        assert_eq!(
            sandbox
                .matches("ensure_detached_host\n\"$ENTRY\" service install --port \"$HOST_PORT\" >/dev/null")
                .count(),
            1
        );
        // Template drift gate (also under --release): the line the insertion anchors on.
        assert_eq!(
            include_str!("remote_bootstrap.sh")
                .matches("\"$ENTRY\" service install --port \"$HOST_PORT\" >/dev/null")
                .count(),
            1
        );
        assert!(!sandbox.contains("template changed"));
        for text in [
            codespace_bootstrap_script(),
            bootstrap_script(HostPlatform::Unix),
        ] {
            assert!(!text.contains("DETACHED"));
            assert!(!text.contains("detached_host"));
        }
        assert!(codespace_bootstrap_script()
            .contains("\"$ENTRY\" service uninstall >/dev/null 2>&1 || true"));
    }
    /// A fake Host that is its own `serve` process: `connection-info` answers (with its pid)
    /// only while `up` exists.
    #[cfg(unix)]
    struct DetachedRun {
        home: tempfile::TempDir,
    }
    #[cfg(unix)]
    impl DetachedRun {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            let home = tempfile::tempdir().unwrap();
            let base = home.path().join(".monocode-host");
            std::fs::create_dir_all(base.join("bin")).unwrap();
            let entry = base.join("bin/monocode-host");
            std::fs::write(
                &entry,
                format!(
                    r#"#!/bin/sh
B={base}
echo "$*" >> "$B/../calls.log"
case "$1" in
  connection-info) [ -f "$B/up" ] && cat "$B/up" || exit 1 ;;
  serve)
    trap 'rm -f "$B/up"; exit 0' TERM
    printf '{{"port":3774,"pid":%s}}\n' "$$" > "$B/up"
    while :; do sleep 1; done ;;
  service)
    if [ "$2" = uninstall ] && [ -f "$B/up" ]; then
      P=$(sed 's/.*"pid":\([0-9]*\).*/\1/' "$B/up")
      kill -TERM "$P" 2>/dev/null
      i=0; while [ -f "$B/up" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i+1)); done
    fi
    [ "$2" = install ] && [ ! -f "$B/up" ] && exit 9
    exit 0 ;;
esac
"#,
                    base = base.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { home }
        }
        fn up(&self) -> std::path::PathBuf {
            self.home.path().join(".monocode-host/up")
        }
        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.home.path().join("calls.log"))
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect()
        }
        fn serves(&self) -> usize {
            self.calls()
                .iter()
                .filter(|c| c.starts_with("serve "))
                .count()
        }
        fn pid(&self) -> Option<u32> {
            let up = std::fs::read_to_string(self.up()).ok()?;
            serde_json::from_str::<serde_json::Value>(&up).ok()?["pid"]
                .as_u64()
                .map(|p| p as u32)
        }
        fn run(&self, token: Option<&str>) {
            use std::io::Write;
            let script = sandbox_bootstrap_script(None).replace(
                "MONOCODE_HOST_INSTALL_CLAUDE=1",
                "MONOCODE_HOST_INSTALL_CLAUDE=0",
            );
            let mut command = Command::new("sh");
            command
                .arg("-s")
                .env("HOME", self.home.path())
                .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
                .env_remove("ANTHROPIC_API_KEY");
            if let Some(token) = token {
                command.env("CLAUDE_CODE_OAUTH_TOKEN", token);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    #[cfg(unix)]
    impl Drop for DetachedRun {
        fn drop(&mut self) {
            if let Some(pid) = self.pid() {
                let _ = Command::new("kill").arg(pid.to_string()).status();
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn detached_bootstrap_starts_the_host_itself_when_it_does_not_answer() {
        let fake = DetachedRun::new();
        fake.run(None);
        let calls = fake.calls();
        let base = fake.home.path().join(".monocode-host");
        assert!(
            calls.contains(&format!("serve --data-dir {} --port 3774", base.display())),
            "{calls:?}"
        );
        // `service install` ran once the Host answered, so the real one returns early.
        let install = calls
            .iter()
            .position(|c| c == "service install --port 3774");
        let serve = calls.iter().position(|c| c.starts_with("serve "));
        assert!(install.is_some() && serve < install, "{calls:?}");
        assert!(fake.pid().is_some());
    }
    #[cfg(unix)]
    #[test]
    fn detached_bootstrap_leaves_an_answering_host_alone() {
        let fake = DetachedRun::new();
        std::fs::write(fake.up(), r#"{"port":3774,"pid":1}"#).unwrap();
        fake.run(None);
        assert_eq!(fake.serves(), 0);
        assert!(!fake.calls().iter().any(|c| c == "service uninstall"));
        // Never leave a pid that is not ours for Drop to signal.
        std::fs::remove_file(fake.up()).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn detached_bootstrap_stops_then_restarts_the_host_when_the_token_changes() {
        let fake = DetachedRun::new();
        fake.run(Some("first"));
        let first = fake.pid().expect("host started");
        // Same token: untouched.
        fake.run(Some("first"));
        assert_eq!(fake.pid(), Some(first));
        assert_eq!(fake.serves(), 1);
        let before = fake.calls().len();
        fake.run(Some("second"));
        let second = fake.pid().expect("host restarted");
        assert_ne!(second, first);
        let calls = fake.calls()[before..].to_vec();
        let pos = |what: &str| calls.iter().position(|c| c.starts_with(what));
        let (stop, serve, install) = (
            pos("service uninstall"),
            pos("serve "),
            pos("service install"),
        );
        assert!(
            stop < serve && serve < install && install.is_some(),
            "{calls:?}"
        );
    }
    #[test]
    fn sandbox_targets_load_and_keep_old_shapes_on_disk() {
        let old: SshTarget =
            serde_json::from_str(r#"{"target":"x","port":null,"remotePort":3774,"codespace":"x"}"#)
                .unwrap();
        assert_eq!(old.sandbox, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("sandbox"));
        let new: SshTarget = serde_json::from_str(
            r#"{"target":"sbx_001abc@sbx_cloud","port":null,"remotePort":3774,"sandbox":{"id":"sbx_001abc","name":"my-box","workspace":"/home/agent/ws","relayPort":47200}}"#,
        )
        .unwrap();
        assert_eq!(new, sandbox_target());
        let again: SshTarget = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert_eq!(again, new);
    }
    #[test]
    fn sandbox_bootstrap_has_the_codespace_flags_and_an_optional_prelude() {
        let plain = sandbox_bootstrap_script(None);
        assert_ne!(plain, codespace_bootstrap_script());
        assert!(plain.starts_with("MONOCODE_HOST_TRACK_TOKEN=1\nMONOCODE_HOST_INSTALL_CLAUDE=1\n"));
        let with = sandbox_bootstrap_script(Some("export X=1"));
        assert!(with.starts_with("export X=1\nMONOCODE_HOST_TRACK_TOKEN=1\n"));
    }
    #[test]
    fn sandbox_platform_is_unix_without_probing() {
        let job = Job::new();
        let askpass = job.askpass().unwrap();
        assert_eq!(
            detect_platform(&sandbox_target(), &job, &askpass).unwrap(),
            HostPlatform::Unix
        );
        assert!(sandbox_target().is_cloud());
    }
}
