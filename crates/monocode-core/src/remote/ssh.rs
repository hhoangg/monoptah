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

/// A Docker Sandbox that runs a devcontainer with the Host inside it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxTarget {
    /// Sandbox id (`sbx_...`), also the ssh user.
    pub id: String,
    /// Sandbox name, for display and the machine endpoint.
    pub name: String,
    /// Workspace folder in the OUTER sandbox, the devcontainer's workspace.
    pub workspace: String,
    /// Loopback port in the outer sandbox where a relay bridges into the
    /// container's Host. The tunnel forwards to this port.
    pub relay_port: u16,
    /// Dev container config, relative to `workspace`, when it is not the default one
    /// (`devcontainer exec` finds its container by workspace and config file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
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

    /// The single remote word that runs `remote` inside the devcontainer. The
    /// script itself travels on stdin, never in argv.
    pub(crate) fn exec_in_container(&self, remote: &[&str]) -> String {
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
}
pub struct Job {
    inner: Mutex<JobData>,
    pub cancelled: AtomicBool,
}
impl Job {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
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
            }),
            cancelled: AtomicBool::new(false),
        })
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
impl Drop for GhOverride {
    fn drop(&mut self) {
        *GH_OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner) = None;
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

fn command(target: &SshTarget, interactive: bool) -> Command {
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
fn command_with_program(
    target: &SshTarget,
    interactive: bool,
    program: &std::ffi::OsStr,
) -> Command {
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
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
        command(target, true),
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
            command(target, true),
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
        run_script_with_command(target, script, job, askpass, command(target, true))
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
            // lives in the devcontainer, not in the outer sandbox.
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
    if !result? {
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

fn bootstrap_script_from_template(platform: HostPlatform, template: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let url = format!("https://github.com/hhoangg/monoptah/releases/download/v{version}");
    match platform {
        // include_str! preserves checkout line endings, including Windows CRLF.
        HostPlatform::Unix => template
            .replace("\r\n", "\n")
            .replace("@@VERSION@@", &shell_quote(version))
            .replace("@@RELEASE@@", &shell_quote(&url)),
        HostPlatform::Windows => template
            .replace("@@VERSION@@", &powershell_quote(version))
            .replace("@@RELEASE@@", &powershell_quote(&url))
            .replace("@@ACL@@", include_str!("windows-acl.ps1")),
    }
}

/// Bootstrap for a sandbox: the codespace flags (the Host tracks the Claude
/// token by hash and installs Claude), plus an optional prelude that is
/// prepended verbatim. The prelude is the seam for per-start secrets, which
/// reach the container on stdin only.
pub fn sandbox_bootstrap_script(prelude: Option<&str>) -> String {
    let script = codespace_bootstrap_script();
    match prelude {
        Some(prelude) => format!("{prelude}\n{script}"),
        None => script,
    }
}

/// Bootstrap for a codespace. Codespace user secrets (CLAUDE_CODE_OAUTH_TOKEN)
/// reach the login shell's environment, and the host's agents only inherit what
/// the host process itself was started with. The script restarts the host once
/// when that token differs from the one the running host was started with
/// (tracked as a hash, never the token itself).
pub fn codespace_bootstrap_script() -> String {
    format!(
        "MONOCODE_HOST_TRACK_TOKEN=1\nMONOCODE_HOST_INSTALL_CLAUDE=1\n{}",
        bootstrap_script(HostPlatform::Unix)
    )
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
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Tunnel {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
    pub fn start(
        target: &SshTarget,
        job: Option<&Arc<Job>>,
        askpass: Option<&Askpass>,
    ) -> Result<Self, String> {
        Self::start_with_command(target, job, askpass, command(target, askpass.is_some()))
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
        // the relay in the outer sandbox (the only place `-L` may point to).
        let remote_port = match target.transport() {
            Transport::Sandbox(sandbox) => sandbox.relay_port,
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
                return Err(format!(
                    "SSH connection failed: {}. Open Settings → Connections and reconnect to check access.",
                    tunnel.stderr.lock().unwrap().trim()
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
    slot: Arc<Mutex<Slot>>,
    generation: u64,
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
        let port = {
            let tunnel = current.tunnel.as_mut()?;
            if !tunnel.alive() {
                current.tunnel = None;
                return None;
            }
            tunnel.port
        };
        Some(TunnelLease {
            endpoint: format!("http://127.0.0.1:{port}"),
            slot: slot.clone(),
            generation: current.generation,
        })
    }
    pub fn endpoint(&self, id: &str, target: &SshTarget) -> Result<TunnelLease, String> {
        let slot = self.slots().entry(id.into()).or_default().clone();
        let mut current = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(tunnel) = current.tunnel.as_mut() {
            if tunnel.alive() {
                return Ok(TunnelLease {
                    endpoint: format!("http://127.0.0.1:{}", tunnel.port),
                    slot: slot.clone(),
                    generation: current.generation,
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
                current.failure = None;
                current.generation = current.generation.wrapping_add(1);
                current.tunnel = Some(tunnel);
                Ok(TunnelLease {
                    endpoint,
                    slot: slot.clone(),
                    generation: current.generation,
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
            slot: old,
            generation: 1,
        };
        let newer = slot(2);
        tunnels.slots().insert("host".into(), newer.clone());
        tunnels.invalidate("host", &old_lease);
        assert!(newer.lock().unwrap().failure.is_some());
        let stale_lease = TunnelLease {
            endpoint: String::new(),
            slot: newer.clone(),
            generation: 1,
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
            let mut command = command(&target, false);
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
        let command = command_with_program(&codespace_target("octo-abc123"), true, "gh".as_ref());
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
        let command = command_with_program(&target, true, "ssh".as_ref());
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
            }),
        }
    }
    #[test]
    fn sandbox_commands_are_plain_ssh_to_the_alias_without_prompts() {
        let command = command_with_program(&sandbox_target(), true, "ssh".as_ref());
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
        let command = command_with_program(&target, true, "ssh".as_ref());
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
        let command = command_with_program(&codespace_target("octo-abc123"), false, "gh".as_ref());
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
        let command = command_with_program(&sandbox_target(), false, script.as_os_str());
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
        assert_eq!(plain, codespace_bootstrap_script());
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
