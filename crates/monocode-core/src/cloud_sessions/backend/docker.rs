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
use crate::codespaces;
use crate::remote::{SandboxMode, SandboxTarget};
use crate::sandboxes::{self, SbxRunner};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const NOT_FOUND: &str = "was not found";

/// How long a short script may take (it may have to resume the sandbox first).
const SCRIPT_SHORT: Duration = Duration::from_secs(120);
/// How long a long script (a git bundle of a big tree) or a copy may take.
const SCRIPT_LONG: Duration = Duration::from_secs(10 * 60);
/// Most bytes [`DockerBackend::copy_from`] accepts, decoded. The archive flow already refuses
/// a stage over 500 MB; this is the backstop for any other caller.
pub const COPY_LIMIT_BYTES: usize = 600 * 1024 * 1024;
/// Most entries one copy may unpack.
const COPY_MAX_ENTRIES: usize = 100_000;
/// Marker that ends a copy's stream, so a cut connection is not taken for a short archive.
const TAR_END_PREFIX: &str = "MONOPTAH_TAR_END=";
/// Prefix of every data line of a copy stream.
const TAR_DATA_PREFIX: &str = "MONOPTAH_B:";

/// Where a script runs: inside the devcontainer, or in the outer sandbox around it. `Outer`
/// carries a path in the outer sandbox whose owner the script runs as.
#[derive(Clone, Copy)]
enum Where<'a> {
    Container,
    Outer(&'a str),
}

/// The remote command that runs `sh -s` in the outer sandbox as the owner of `path` (the
/// workspace, whose `.git` the script reads and whose stage it writes), so file ownership
/// and permissions stay right. Falls back to the ssh user when the owner is the same, `stat`
/// fails or passwordless sudo is missing. `path` must already be validated.
fn outer_command(path: &str) -> String {
    let script = format!(
        "ws={}; u=$(stat -c %u \"$ws\" 2>/dev/null || true); if [ -n \"$u\" ] && [ \"$u\" != \"$(id -u)\" ] && sudo -n true 2>/dev/null; then exec sudo -n -u \"#$u\" sh -s; else exec sh -s; fi",
        crate::remote::shell_quote(path)
    );
    format!("sh -c {}", crate::remote::shell_quote(&script))
}

pub struct DockerBackend {
    runner: Arc<dyn SbxRunner>,
    ids: Mutex<HashMap<String, String>>,
    /// The ssh seam of [`Self::run_script`] and [`Self::copy_from`]; without it they refuse.
    shell: Option<Arc<dyn SandboxShell>>,
    /// Where each sandbox's devcontainer is, by name (set by the start and reconnect flows).
    targets: Mutex<HashMap<String, SandboxTarget>>,
    /// Most decoded bytes one [`Self::copy_from`] accepts.
    copy_limit: usize,
    /// Entries the last copies skipped (links), taken by [`CloudBackend::take_copy_notes`].
    notes: Mutex<Vec<String>>,
}

impl DockerBackend {
    pub fn new(runner: Arc<dyn SbxRunner>) -> Self {
        Self {
            runner,
            ids: Mutex::new(HashMap::new()),
            shell: None,
            targets: Mutex::new(HashMap::new()),
            copy_limit: COPY_LIMIT_BYTES,
            notes: Mutex::new(Vec::new()),
        }
    }
    /// Uses `bytes` as the copy size limit instead of [`COPY_LIMIT_BYTES`].
    pub fn with_copy_limit(mut self, bytes: usize) -> Self {
        self.copy_limit = bytes;
        self
    }
    /// Lets [`Self::run_script`] and [`Self::copy_from`] work. They ssh into the sandbox
    /// (which resumes a stopped one), so only user actions may call them.
    pub fn with_shell(mut self, shell: Arc<dyn SandboxShell>) -> Self {
        self.shell = Some(shell);
        self
    }
    /// Remembers where the devcontainer of `target.name` is.
    pub fn remember_target(&self, target: &SandboxTarget) {
        self.remember(&target.name, &target.id);
        if let Ok(mut targets) = self.targets.lock() {
            targets.insert(target.name.clone(), target.clone());
        }
    }
    fn target_of(&self, name: &str) -> Result<(Arc<dyn SandboxShell>, SandboxTarget), String> {
        let shell = self
            .shell
            .clone()
            .ok_or_else(|| "Docker sandboxes are not available.".to_string())?;
        let target = self
            .targets
            .lock()
            .ok()
            .and_then(|targets| targets.get(name).cloned())
            .ok_or_else(|| {
                "This sandbox is not connected. Reconnect the session first.".to_string()
            })?;
        Ok((shell, target))
    }
    /// Runs `stdin` as `sh -l -s` inside the container (or in the workspace of a plain
    /// sandbox, which has no container), lines to `on_line`.
    fn exec(
        &self,
        name: &str,
        stdin: &str,
        on_line: &mut dyn FnMut(&str),
        abort: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<ShellOutput, String> {
        self.exec_at(Where::Container, name, stdin, on_line, abort, timeout)
    }
    /// Runs `stdin` as `sh -s` directly in the OUTER sandbox (no container exec), lines to
    /// `on_line`. Needs only the sandbox id, so it works when the container is gone.
    fn exec_outer(
        &self,
        name: &str,
        owner_path: &str,
        stdin: &str,
        on_line: &mut dyn FnMut(&str),
        abort: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<ShellOutput, String> {
        self.exec_at(
            Where::Outer(owner_path),
            name,
            stdin,
            on_line,
            abort,
            timeout,
        )
    }
    fn exec_at(
        &self,
        at: Where<'_>,
        name: &str,
        stdin: &str,
        on_line: &mut dyn FnMut(&str),
        abort: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<ShellOutput, String> {
        let (shell, id, remote) = match at {
            Where::Container => {
                let (shell, target) = self.target_of(name)?;
                let remote = target.exec_in_container(&["sh", "-l", "-s"]);
                (shell, target.id, remote)
            }
            Where::Outer(owner_path) => {
                codespaces::valid_remote_path(owner_path)?;
                let shell = self
                    .shell
                    .clone()
                    .ok_or_else(|| "Docker sandboxes are not available.".to_string())?;
                (shell, self.id_of(name)?, outer_command(owner_path))
            }
        };
        shell
            .run(&id, &remote, stdin, on_line, abort, timeout)
            .map_err(|e| sandboxes::redact(&e))
    }
    /// Whether the remembered target of `name` runs without a devcontainer.
    fn is_plain(&self, name: &str) -> bool {
        self.targets
            .lock()
            .ok()
            .and_then(|targets| targets.get(name).map(|t| t.mode == SandboxMode::Plain))
            .unwrap_or(false)
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
    /// ssh into the sandbox and `devcontainer exec` into the container (a plain sandbox: run in
    /// its workspace), the script on stdin.
    /// Resumes a stopped sandbox: user-initiated work only.
    fn run_script(&self, name: &str, script: &str, long: bool) -> Result<String, String> {
        let timeout = if long { SCRIPT_LONG } else { SCRIPT_SHORT };
        let out = self.exec(name, script, &mut |_| {}, &|| false, timeout)?;
        if out.success {
            Ok(out.stdout)
        } else {
            Err(script_failure(&out))
        }
    }
    /// Streams `remote` (a file or folder in the container) as a base64 tar over the ssh
    /// stdout and unpacks it into the existing absolute folder `local`, like `cp -r remote
    /// local`. Entries are validated (relative, no `..`, files and folders only: links are
    /// skipped) and the size and entry count are limited. A missing `remote` fails with
    /// `<remote>: No such file or directory`, which
    /// [`crate::codespaces::is_missing_path`] recognises.
    fn copy_from(&self, name: &str, remote: &str, local: &Path) -> Result<(), String> {
        self.copy_at(Where::Container, name, remote, local)
    }
    fn take_copy_notes(&self) -> Vec<String> {
        self.notes
            .lock()
            .map(|mut notes| std::mem::take(&mut *notes))
            .unwrap_or_default()
    }
    /// The same evidence folder and agent guidance as a codespace, set up in the container.
    fn prepare_evidence(&self, name: &str, workspace: &str) -> Result<(), String> {
        // A plain sandbox has no `/workspaces/<repo>`: the Host works in the outer workspace.
        let script = if self.is_plain(name) {
            codespaces::evidence_script_at(workspace)?
        } else {
            codespaces::evidence_script_for(workspace)?
        };
        let out = self.exec(name, &script, &mut |_| {}, &|| false, SCRIPT_SHORT)?;
        if out.success {
            Ok(())
        } else {
            Err(script_failure(&out))
        }
    }
}

impl DockerBackend {
    /// Runs `script` (on stdin, never in argv) with `sh -s` in the OUTER sandbox as the owner
    /// of `owner_path`, for the files-only archive of a session whose container was
    /// recreated or whose Host is gone. It resumes a stopped sandbox: only the
    /// user-initiated archive may call it.
    pub(crate) fn run_outer_script(
        &self,
        name: &str,
        owner_path: &str,
        script: &str,
        long: bool,
    ) -> Result<String, String> {
        let timeout = if long { SCRIPT_LONG } else { SCRIPT_SHORT };
        let out = self.exec_outer(name, owner_path, script, &mut |_| {}, &|| false, timeout)?;
        if out.success {
            Ok(out.stdout)
        } else {
            Err(script_failure(&out))
        }
    }
    /// [`CloudBackend::copy_from`] for a path in the OUTER sandbox (the same verified tar
    /// stream, run without `devcontainer exec`). Same restrictions on who may call it as
    /// [`Self::run_outer_script`].
    pub(crate) fn copy_from_outer(
        &self,
        name: &str,
        owner_path: &str,
        remote: &str,
        local: &Path,
    ) -> Result<(), String> {
        self.copy_at(Where::Outer(owner_path), name, remote, local)
    }
    /// Streams `remote` as a base64 tar and unpacks it into `local`, see
    /// [`CloudBackend::copy_from`].
    fn copy_at(&self, at: Where<'_>, name: &str, remote: &str, local: &Path) -> Result<(), String> {
        codespaces::valid_remote_path(remote)?;
        if !local.is_absolute() {
            return Err("Local destination must be an absolute path".into());
        }
        if !local.is_dir() {
            return Err("Local destination is not a folder".into());
        }
        let script = COPY_SCRIPT.replace("@PATH@", remote);
        // The archive streams into a file next to the destination, never into memory.
        let archive = local.join(format!(
            ".monoptah-copy-{}-{}.tar",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let result = self.copy_into(at, name, &script, &archive, local);
        let _ = std::fs::remove_file(&archive);
        result
    }
    /// Runs the copy script and writes the decoded stream to `archive`, then unpacks it.
    fn copy_into(
        &self,
        at: Where<'_>,
        name: &str,
        script: &str,
        archive: &Path,
        local: &Path,
    ) -> Result<(), String> {
        use base64::Engine as _;
        use std::io::Write as _;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(archive)
            .map_err(|e| format!("Could not stage the copy: {e}"))?;
        let writer = std::cell::RefCell::new(std::io::BufWriter::new(file));
        let written = std::cell::Cell::new(0usize);
        let expected = std::cell::Cell::new(None::<usize>);
        let problem = std::cell::RefCell::new(None::<String>);
        let out = {
            let mut on_line = |line: &str| {
                if problem.borrow().is_some() {
                    return;
                }
                let line = line.trim_end();
                if let Some(count) = line.strip_prefix(TAR_END_PREFIX) {
                    expected.set(count.parse().ok());
                    return;
                }
                // Only data-prefixed lines carry the archive; any other line (ssh noise,
                // stderr) is ignored.
                let Some(data) = line.strip_prefix(TAR_DATA_PREFIX) else {
                    return;
                };
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(chunk) => {
                        if written.get() + chunk.len() > self.copy_limit {
                            *problem.borrow_mut() = Some(format!(
                                "The copy is over the {} MB limit.",
                                self.copy_limit / (1024 * 1024)
                            ));
                        } else if let Err(e) = writer.borrow_mut().write_all(&chunk) {
                            *problem.borrow_mut() = Some(format!("Could not stage the copy: {e}"));
                        } else {
                            written.set(written.get() + chunk.len());
                        }
                    }
                    Err(_) => {
                        *problem.borrow_mut() =
                            Some("The copy arrived damaged (bad base64).".to_string());
                    }
                }
            };
            self.exec_at(
                at,
                name,
                script,
                &mut on_line,
                &|| problem.borrow().is_some(),
                SCRIPT_LONG,
            )
        };
        if let Some(problem) = problem.take() {
            return Err(problem);
        }
        let out = out?;
        if !out.success {
            if let Some(tool) = out
                .stderr
                .lines()
                .find_map(|l| l.trim().strip_prefix("MONOPTAH_MISSING_TOOL="))
            {
                return Err(format!(
                    "The sandbox image has no `{tool}`, which copying files out needs."
                ));
            }
            return Err(script_failure(&out));
        }
        writer
            .borrow_mut()
            .flush()
            .map_err(|e| format!("Could not stage the copy: {e}"))?;
        match expected.get() {
            None => return Err("The copy was cut off before it finished.".into()),
            Some(count) if count != written.get() => {
                return Err(format!(
                    "The copy arrived incomplete ({} of {count} bytes).",
                    written.get()
                ))
            }
            Some(_) => {}
        }
        let skipped = unpack(
            std::fs::File::open(archive).map_err(|e| e.to_string())?,
            local,
        )?;
        if let Ok(mut notes) = self.notes.lock() {
            notes.extend(skipped);
        }
        Ok(())
    }
}

/// Streams a path as [`TAR_DATA_PREFIX`] base64 lines and ends with `MONOPTAH_TAR_END=<bytes>`. `@PATH@` is validated by
/// [`codespaces::valid_remote_path`] before it is substituted. The archive goes through a
/// temporary file so a failing `tar` is seen (tar exit 1 only means a file changed).
const COPY_SCRIPT: &str = r#"set -eu
p='@PATH@'
for tool in tar base64 mktemp wc sed; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "MONOPTAH_MISSING_TOOL=$tool" >&2
    exit 45
  fi
done
if [ ! -e "$p" ] && [ ! -L "$p" ]; then
  echo "$p: No such file or directory" >&2
  exit 44
fi
tmp=$(mktemp)
enc=$(mktemp)
trap 'rm -f "$tmp" "$enc"' EXIT
cd "$(dirname "$p")"
rc=0
tar -cf "$tmp" -- "$(basename "$p")" || rc=$?
if [ "$rc" -gt 1 ]; then exit "$rc"; fi
base64 < "$tmp" > "$enc"
n=$(wc -c < "$tmp" | tr -d ' ')
sed 's/^/MONOPTAH_B:/' < "$enc"
echo "MONOPTAH_TAR_END=$n"
"#;

/// Last lines of a failed script's output, redacted, as one error sentence.
fn script_failure(out: &ShellOutput) -> String {
    let text = if out.stderr.trim().is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(4)..].join("\n");
    let tail: String = sandboxes::redact(&tail).chars().take(600).collect();
    if tail.is_empty() {
        "The command in the sandbox failed".into()
    } else {
        tail
    }
}

/// Unpacks `archive` into `dest`: relative paths only (the `tar` crate refuses anything that
/// would leave `dest`), regular files and folders only (a link is skipped, never followed, and
/// reported in the returned notes), at most [`COPY_MAX_ENTRIES`] entries.
fn unpack(archive: impl std::io::Read, dest: &Path) -> Result<Vec<String>, String> {
    let mut archive = tar::Archive::new(archive);
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    let mut count = 0usize;
    let mut skipped = Vec::new();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        count += 1;
        if count > COPY_MAX_ENTRIES {
            return Err("The copy has too many files.".into());
        }
        let kind = entry.header().entry_type();
        let relative = entry.path().map_err(|e| e.to_string())?.into_owned();
        if !(kind.is_file() || kind.is_dir()) {
            skipped.push(format!(
                "{} (link or special file) was not copied",
                relative.display()
            ));
            continue;
        }
        let clean = relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !clean || relative.as_os_str().is_empty() {
            return Err(format!(
                "The copy has an unsafe path: {}",
                relative.display()
            ));
        }
        if !entry.unpack_in(dest).map_err(|e| e.to_string())? {
            return Err(format!(
                "The copy has an unsafe path: {}",
                relative.display()
            ));
        }
    }
    Ok(skipped)
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
