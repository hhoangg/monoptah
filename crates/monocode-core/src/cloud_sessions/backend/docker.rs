//! [`CloudBackend`] over Docker cloud sandboxes (`sbx`), plus the ssh seam the start flow
//! uses to run scripts in a sandbox.
//!
//! * [`DockerBackend`] only ever uses the read-only and lifecycle verbs of
//!   [`crate::sandboxes`]. It never runs ssh, so asking for a state can not resume (and
//!   bill) a stopped sandbox. The trait addresses a workspace by *name* while `sbx` wants
//!   the *id*, so it keeps a name -> id cache and falls back to
//!   [`sandboxes::find_by_name`] (a list call) for a name it has not seen.
//! * [`SandboxShell`] runs one command in a sandbox over `ssh <id>@sbx_cloud`, script on
//!   stdin. Only [`CloudSessions::run_start`](crate::cloud_sessions::CloudSessions::run_start)
//!   and rebuild (user actions) may use it. [`SystemShell`] is the real one; tests inject a
//!   fake.
use super::{CloudBackend, CloudBackendKind};
use crate::cloud_sessions::CloudState;
use crate::sandboxes::{self, SbxRunner};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const NOT_FOUND: &str = "was not found";
const NOT_SUPPORTED: &str = "is not supported for sandboxes yet.";

pub struct DockerBackend {
    runner: Arc<dyn SbxRunner>,
    ids: Mutex<HashMap<String, String>>,
}

impl DockerBackend {
    pub fn new(runner: Arc<dyn SbxRunner>) -> Self {
        Self {
            runner,
            ids: Mutex::new(HashMap::new()),
        }
    }
    /// Remembers the id of the sandbox called `name`, so later calls skip the list lookup.
    pub fn remember(&self, name: &str, id: &str) {
        if let Ok(mut ids) = self.ids.lock() {
            ids.insert(name.into(), id.into());
        }
    }
    fn forget(&self, name: &str) {
        if let Ok(mut ids) = self.ids.lock() {
            ids.remove(name);
        }
    }
    /// The id of sandbox `name`: remembered, else looked up by exact name.
    fn id_of(&self, name: &str) -> Result<String, String> {
        if let Some(id) = self.ids.lock().ok().and_then(|ids| ids.get(name).cloned()) {
            return Ok(id);
        }
        match sandboxes::find_by_name(&*self.runner, name)? {
            Some(info) => {
                self.remember(name, &info.id);
                Ok(info.id)
            }
            None => Err(format!("Sandbox {name} {NOT_FOUND}")),
        }
    }
    /// `sbx` said the remembered id is gone: forget it so a recreated name is looked up again.
    fn after_missing(&self, name: &str, id: &str, error: &str) {
        if sandboxes::is_deleted(error, id) {
            self.forget(name);
        }
    }
}

impl CloudBackend for DockerBackend {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::DockerSandbox
    }
    fn display_state(&self, raw: &str) -> CloudState {
        sandboxes::display_state(raw)
    }
    fn is_stopped(&self, raw: &str) -> bool {
        sandboxes::is_stopped(raw)
    }
    fn is_deleted_state(&self, raw: &str) -> bool {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "deleted" | "terminated"
        )
    }
    fn is_missing_error(&self, error: &str) -> bool {
        // Only the CLI's exact "sandbox '<ref>' not found" answer, or our own lookup miss,
        // proves absence: a false positive makes the app forget a sandbox that still bills.
        error.lines().any(|line| {
            let line = line.trim();
            let line = line.strip_prefix("error:").map_or(line, str::trim);
            let lower = line.to_ascii_lowercase();
            (lower.starts_with("sandbox '") && lower.ends_with("' not found"))
                || (lower.starts_with("sandbox ") && lower.ends_with(NOT_FOUND))
        })
    }
    fn state(&self, name: &str) -> Result<String, String> {
        let id = self.id_of(name)?;
        sandboxes::state(&*self.runner, &id).inspect_err(|e| self.after_missing(name, &id, e))
    }
    fn exists(&self, name: &str) -> Result<bool, String> {
        match self.state(name) {
            Ok(raw) => Ok(!self.is_deleted_state(&raw)),
            Err(error) if self.is_missing_error(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn stop(&self, name: &str) -> Result<(), String> {
        let id = self.id_of(name)?;
        sandboxes::stop(&*self.runner, &id)
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        let id = self.id_of(name)?;
        sandboxes::delete(&*self.runner, &id)?;
        self.forget(name);
        Ok(())
    }
    fn rebuild(&self, _name: &str) -> Result<(), String> {
        Err("Rebuild a sandbox session from its start panel.".into())
    }
    fn web_url(&self, _name: &str) -> Option<Result<String, String>> {
        None
    }
    fn run_script(&self, _name: &str, _script: &str, _long: bool) -> Result<String, String> {
        Err(format!("Running a script {NOT_SUPPORTED}"))
    }
    fn copy_from(&self, _name: &str, _remote: &str, _local: &Path) -> Result<(), String> {
        Err(format!("Copying files {NOT_SUPPORTED}"))
    }
    fn prepare_evidence(&self, _name: &str, _workspace: &str) -> Result<(), String> {
        // Nothing to prepare until copying exists; not an error, the session still opens.
        Ok(())
    }
}

// ----------------------------------------------------------------------- shell

/// What a command run through [`SandboxShell`] produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Why a [`SandboxShell::run`] ended early.
pub const SHELL_ABORTED: &str = "The command was cancelled.";

/// Runs commands in a sandbox over `ssh <id>@sbx_cloud`. Any call resumes a stopped
/// sandbox, so it is for user-initiated work only.
pub trait SandboxShell: Send + Sync {
    /// Runs `remote` (one shell command word list, run by the sandbox user's shell) with
    /// `stdin` on its standard input. Every output line (stdout and stderr) reaches
    /// `on_line` as it arrives, on the calling thread. `abort` is polled; when it turns
    /// true the command is killed and the call returns `Err(`[`SHELL_ABORTED`]`)`. `Err`
    /// otherwise means ssh itself could not run or `timeout` passed; a command that ran
    /// and failed is `Ok` with `success == false`.
    fn run(
        &self,
        id: &str,
        remote: &str,
        stdin: &str,
        on_line: &mut dyn FnMut(&str),
        abort: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<ShellOutput, String>;
}

/// Real ssh.
pub struct SystemShell {
    program: std::ffi::OsString,
}

impl Default for SystemShell {
    fn default() -> Self {
        Self {
            program: "ssh".into(),
        }
    }
}

impl SystemShell {
    /// Uses `program` instead of `ssh` (tests point it at a fake).
    pub fn with_program(program: impl Into<std::ffi::OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }
    fn command(&self, id: &str, remote: &str) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                // Resuming a stopped sandbox through the sbx ProxyCommand can take a while.
                "-o",
                "ConnectTimeout=120",
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
                "--",
            ])
            .arg(sandboxes::ssh_destination(id))
            .arg(remote)
            .env("LC_ALL", "C")
            // The alias's ProxyCommand runs `sbx`, which a Finder-launched app cannot find on
            // launchd's bare PATH.
            .env("PATH", crate::scm::terminal_search_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::configure_child(&mut command);
        command
    }
}

impl SandboxShell for SystemShell {
    fn run(
        &self,
        id: &str,
        remote: &str,
        stdin: &str,
        on_line: &mut dyn FnMut(&str),
        abort: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<ShellOutput, String> {
        sandboxes::validate_sandbox_id(id)?;
        let mut child = self
            .command(id, remote)
            .spawn()
            .map_err(|e| format!("Could not start OpenSSH: {e}"))?;
        let (tx, rx) = mpsc::channel::<(bool, String)>();
        let reader = |pipe: Option<Box<dyn Read + Send>>, is_err: bool| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Some(pipe) = pipe {
                    for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                        let line = String::from_utf8_lossy(&line)
                            .trim_end_matches('\r')
                            .to_string();
                        if tx.send((is_err, line)).is_err() {
                            break;
                        }
                    }
                }
            })
        };
        let out = reader(child.stdout.take().map(|p| Box::new(p) as _), false);
        let err = reader(child.stderr.take().map(|p| Box::new(p) as _), true);
        drop(tx);
        if let Some(mut pipe) = child.stdin.take() {
            let text = zeroize::Zeroizing::new(stdin.to_string());
            std::thread::spawn(move || {
                // A closed pipe means the command exited early; its status says why.
                let _ = pipe.write_all(text.as_bytes());
            });
        }
        let deadline = Instant::now() + timeout;
        let mut stdout = String::new();
        let mut stderr = String::new();
        let kill = |child: &mut std::process::Child| {
            let _ = child.kill();
            let _ = child.wait();
        };
        let mut last_check = Instant::now();
        let status = loop {
            if last_check.elapsed() >= Duration::from_millis(200) {
                last_check = Instant::now();
                if abort() {
                    kill(&mut child);
                    return Err(SHELL_ABORTED.into());
                }
                if Instant::now() >= deadline {
                    kill(&mut child);
                    return Err(
                        "The sandbox did not answer in time. Check your network connection.".into(),
                    );
                }
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok((is_err, line)) => {
                    on_line(&line);
                    let sink = if is_err { &mut stderr } else { &mut stdout };
                    if sink.len() < 256 * 1024 {
                        sink.push_str(&line);
                        sink.push('\n');
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                // Both pipes closed: the command is finishing.
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break child.wait().map_err(|e| e.to_string())?;
                }
            }
        };
        let _ = (out.join(), err.join());
        Ok(ShellOutput {
            success: status.success(),
            stdout,
            stderr,
        })
    }
}
