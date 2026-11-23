//! The one place that finds `node` and `npx`.
//!
//! A GUI app started from the Dock or Finder does not inherit the login shell's PATH, so a node
//! from nvm, fnm, volta, asdf, mise or a custom prefix is invisible to a plain PATH lookup. The
//! lookup order is: the override from Settings, the login shell's own `command -v npx`, the
//! well-known install locations, then PATH. Everything that spawns `npx` or `node` (the browser
//! prerequisite check and the Playwright MCP server) goes through [`resolve`] / [`cached`] and
//! puts [`NodeTools::path_with`] in the child's PATH.
//!
//! All I/O goes through [`Probe`], so the order and the parsing are tested with a fake file
//! system, home, environment and shell. The real shell runs only in [`resolve`], which blocks
//! for up to [`LOGIN_SHELL_TIMEOUT`]: call it from a worker, never from the UI thread.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

#[cfg(all(test, unix))]
mod resolver_tests;
#[cfg(test)]
mod tests;

pub const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long "nothing found" is trusted before [`resolve`] looks again.
const MISS_TTL: Duration = Duration::from_secs(60);
/// The fixed command the login shell runs. Nothing from the user is put into it.
#[cfg(unix)]
const LOGIN_SHELL_COMMAND: &str = "command -v npx";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeSource {
    /// The path chosen in Settings.
    Override,
    /// The user's login shell found it, as their terminal does.
    LoginShell,
    /// A well-known install location (Homebrew, nvm, fnm, volta, asdf, mise, nodenv, n).
    WellKnown,
    /// The app's own PATH.
    Path,
}
impl NodeSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Override => "set in Settings",
            Self::LoginShell => "your login shell",
            Self::WellKnown => "a known install location",
            Self::Path => "PATH",
        }
    }
}

/// A usable Node.js: `npx` and `node` side by side in `bin_dir`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeTools {
    pub npx: PathBuf,
    pub node: PathBuf,
    pub bin_dir: PathBuf,
    pub source: NodeSource,
}
impl NodeTools {
    /// `base` with the node folder first, so `npx` finds its own `node` (and shims such as
    /// asdf's and mise's find their runtime).
    pub fn path_with(&self, base: &str) -> String {
        let parts = std::iter::once(self.bin_dir.clone()).chain(std::env::split_paths(base));
        std::env::join_paths(parts)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| base.to_owned())
    }
}

/// Everything the resolver reads from the machine.
pub trait Probe {
    fn home(&self) -> Option<PathBuf>;
    /// The login shell: `$SHELL`, or the platform default. Not yet checked.
    fn shell(&self) -> Option<PathBuf>;
    fn path_var(&self) -> String;
    /// An existing file the user may run.
    fn is_executable(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn read_to_string(&self, path: &Path) -> Option<String>;
    /// Entry names of a folder, empty when it cannot be read.
    fn list_dir(&self, path: &Path) -> Vec<String>;
    /// Runs [`LOGIN_SHELL_COMMAND`] in `shell` and returns its stdout. `None` when the shell did
    /// not start, did not finish in [`LOGIN_SHELL_TIMEOUT`] (it is killed) or failed.
    fn run_login_shell(&self, shell: &Path) -> Option<String>;
}

/// The real machine.
pub struct RealProbe;
impl Probe for RealProbe {
    fn home(&self) -> Option<PathBuf> {
        crate::scm::shell::dirs_home().map(PathBuf::from)
    }
    fn shell(&self) -> Option<PathBuf> {
        if cfg!(windows) {
            return None;
        }
        let shell = std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        shell.or_else(|| {
            Some(PathBuf::from(if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/bash"
            }))
        })
    }
    fn path_var(&self) -> String {
        std::env::var("PATH").unwrap_or_default()
    }
    fn is_executable(&self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            path.is_file()
        }
    }
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
    fn list_dir(&self, path: &Path) -> Vec<String> {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }
    #[cfg(unix)]
    fn run_login_shell(&self, shell: &Path) -> Option<String> {
        use std::io::Read;
        use std::process::{Command, Stdio};
        // `-i` as well as `-l`: zsh reads `.zshrc` only when interactive, and version managers
        // start from there.
        let mut command = Command::new(shell);
        command
            .args(["-l", "-i", "-c", LOGIN_SHELL_COMMAND])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(home) = self.home().filter(|h| h.is_dir()) {
            command.current_dir(home);
        }
        crate::process::isolate(&mut command);
        let mut child = command.spawn().ok()?;
        let pid = child.id();
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        };
        // The pipe is read on its own thread into a shared buffer and never waited for: a daemon
        // started by the shell's rc files can keep it open long after the shell is gone, and what
        // was written by then is still the answer.
        let output = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = output.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut chunk) {
                let mut buf = sink.lock().unwrap_or_else(|e| e.into_inner());
                if n == 0 || buf.len() >= 64 * 1024 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
        });
        let deadline = Instant::now() + LOGIN_SHELL_TIMEOUT;
        let finished = loop {
            match child.try_wait() {
                Ok(Some(_)) => break true,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => break false,
            }
        };
        if !finished {
            // The shell leads its own process group (see `isolate`): kill all of it, then reap.
            // SAFETY: plain signal to a process group this function created.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        // A moment for the reader to take what the exited shell wrote.
        std::thread::sleep(Duration::from_millis(50));
        let bytes = output.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
    #[cfg(not(unix))]
    fn run_login_shell(&self, _: &Path) -> Option<String> {
        None
    }
}

// ---- Resolution ----

/// `npx` and `node` in `dir`, both executable.
fn tools_in(probe: &dyn Probe, dir: &Path, source: NodeSource) -> Option<NodeTools> {
    if dir.as_os_str().is_empty() {
        return None;
    }
    let find = |name: &str| {
        let names: &[&str] = if cfg!(windows) {
            &["cmd", "exe", "bat"]
        } else {
            &[""]
        };
        names.iter().find_map(|ext| {
            let file = if ext.is_empty() {
                name.to_owned()
            } else {
                format!("{name}.{ext}")
            };
            let path = dir.join(file);
            probe.is_executable(&path).then_some(path)
        })
    };
    Some(NodeTools {
        npx: find("npx")?,
        node: find("node")?,
        bin_dir: dir.to_path_buf(),
        source,
    })
}

/// Checks the path from Settings. A folder must hold `npx` and `node`; a file is taken as `npx`
/// and `node` must sit beside it. The text is only ever used as a path, never run through a shell.
pub fn validate_override(probe: &dyn Probe, raw: &str) -> Result<NodeTools, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains(['\0', '\n', '\r']) {
        return Err("Enter the full path of npx or of the folder that holds it.".into());
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(
            "The path must be absolute, for example /Users/you/.nvm/versions/node/v22.1.0/bin."
                .into(),
        );
    }
    if probe.is_dir(path) {
        return tools_in(probe, path, NodeSource::Override)
            .ok_or_else(|| "That folder does not hold both npx and node.".into());
    }
    if !probe.is_executable(path) {
        return Err("That path does not exist or is not executable.".into());
    }
    let named_npx = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| matches!(n, "npx" | "npx.cmd" | "npx.exe" | "npx.bat"));
    if !named_npx {
        return Err("The file must be npx, or choose the folder that holds it.".into());
    }
    let dir = path.parent().unwrap_or(Path::new(""));
    let tools = tools_in(probe, dir, NodeSource::Override)
        .ok_or("node must sit in the same folder as npx.")?;
    Ok(NodeTools {
        npx: path.to_path_buf(),
        ..tools
    })
}

/// The last line of the shell's output that is an absolute path to a runnable file. A login
/// shell prints banners, `motd` and prompt noise around the answer.
fn parse_shell_output(probe: &dyn Probe, output: &str) -> Option<PathBuf> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .map(Path::new)
        .find(|p| p.is_absolute() && probe.is_executable(p))
        .map(Path::to_path_buf)
}

fn from_login_shell(probe: &dyn Probe) -> Option<NodeTools> {
    let shell = probe.shell().filter(|s| s.is_absolute())?;
    if !probe.is_executable(&shell) {
        return None;
    }
    let npx = parse_shell_output(probe, &probe.run_login_shell(&shell)?)?;
    tools_in(probe, npx.parent()?, NodeSource::LoginShell)
}

/// `v22.1.0` as a sortable tuple; anything else is not a version.
fn parse_version(name: &str) -> Option<(u64, u64, u64)> {
    let mut parts = name.strip_prefix('v').unwrap_or(name).split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}
/// Installed version folders, newest first.
fn versions_newest_first(probe: &dyn Probe, dir: &Path) -> Vec<String> {
    let mut found: Vec<_> = probe
        .list_dir(dir)
        .into_iter()
        .filter_map(|n| parse_version(&n).map(|v| (v, n)))
        .collect();
    found.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
    found.into_iter().map(|(_, n)| n).collect()
}

/// nvm's default version folder name, following aliases (`default` -> `lts/*` -> `lts/iron` ->
/// `v20.11.1`). `node` and `stable` mean the newest installed.
fn nvm_default(probe: &dyn Probe, root: &Path, installed: &[String]) -> Option<String> {
    let mut value = probe
        .read_to_string(&root.join("alias/default"))?
        .trim()
        .to_owned();
    for _ in 0..6 {
        // An alias is a name under `alias/`: never absolute, never leaving the folder.
        let relative = Path::new(&value)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        if value.is_empty() || !relative || value.contains('\\') {
            return None;
        }
        if matches!(value.as_str(), "node" | "stable") {
            return installed.first().cloned();
        }
        if let Some(next) = probe.read_to_string(&root.join("alias").join(&value)) {
            let next = next.trim().to_owned();
            if next != value {
                value = next;
                continue;
            }
        }
        if value.starts_with("lts/") {
            // An LTS alias with no file: the newest installed is the closest honest answer.
            return installed.first().cloned();
        }
        // A version or a prefix of one ("v22", "22.1"): the newest installed match.
        let prefix = value.strip_prefix('v').unwrap_or(&value).to_owned();
        return installed
            .iter()
            .find(|name| {
                let name = name.strip_prefix('v').unwrap_or(name);
                name == prefix || name.starts_with(&format!("{prefix}."))
            })
            .cloned();
    }
    None
}

/// Folders to try, best first: the version a new terminal gets, then the newest installed.
fn nvm_dirs(probe: &dyn Probe, home: &Path) -> Vec<PathBuf> {
    let root = home.join(".nvm");
    let versions = root.join("versions/node");
    let installed = versions_newest_first(probe, &versions);
    let mut dirs = Vec::new();
    if let Some(default) = nvm_default(probe, &root, &installed) {
        dirs.push(versions.join(default).join("bin"));
    }
    dirs.extend(installed.iter().map(|v| versions.join(v).join("bin")));
    dirs
}

fn fnm_dirs(probe: &dyn Probe, home: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for root in [
        home.join(".fnm"),
        home.join(".local/share/fnm"),
        home.join("Library/Application Support/fnm"),
    ] {
        // `aliases/default` is a link to the version's `installation` folder.
        dirs.push(root.join("aliases/default/bin"));
        let versions = root.join("node-versions");
        for v in versions_newest_first(probe, &versions) {
            dirs.push(versions.join(v).join("installation/bin"));
        }
    }
    dirs
}

/// The install locations to look in, in order.
fn well_known_dirs(probe: &dyn Probe) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        // PATH already carries Program Files\nodejs there: nothing more to look at.
        return dirs;
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(home) = probe.home() {
        dirs.extend(nvm_dirs(probe, &home));
        dirs.extend(fnm_dirs(probe, &home));
        dirs.extend(
            [
                ".volta/bin",
                ".asdf/shims",
                ".local/share/mise/shims",
                ".nodenv/shims",
                "n/bin",
            ]
            .map(|d| home.join(d)),
        );
    }
    if !cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/usr/bin"));
    }
    dirs
}

/// Finds node and npx. An override that does not validate is skipped here (Settings shows its
/// error through [`validate_override`]), so a typo never leaves the app with no node at all.
pub fn resolve_with(probe: &dyn Probe, override_path: Option<&str>) -> Option<NodeTools> {
    if let Some(tools) = override_path
        .filter(|p| !p.trim().is_empty())
        .and_then(|p| validate_override(probe, p).ok())
    {
        return Some(tools);
    }
    if let Some(tools) = from_login_shell(probe) {
        return Some(tools);
    }
    if let Some(tools) = well_known_dirs(probe)
        .into_iter()
        .find_map(|dir| tools_in(probe, &dir, NodeSource::WellKnown))
    {
        return Some(tools);
    }
    std::env::split_paths(&probe.path_var()).find_map(|dir| tools_in(probe, &dir, NodeSource::Path))
}

// ---- The app's shared state ----

struct Cached {
    tools: Option<NodeTools>,
    at: Instant,
}
/// The override, the remembered lookup and the bookkeeping that keeps a slow lookup from
/// overwriting a newer answer. One instance serves the app; tests make their own.
struct State {
    over: RwLock<Option<String>>,
    cache: Mutex<Option<Cached>>,
    /// Held while the shell runs, so two callers do not start two shells.
    resolving: Mutex<()>,
    /// Bumped by every [`State::recheck`] and override change: a lookup that started before
    /// the bump is stale and is not remembered.
    generation: AtomicU64,
}
impl State {
    const fn new() -> Self {
        Self {
            over: RwLock::new(None),
            cache: Mutex::new(None),
            resolving: Mutex::new(()),
            generation: AtomicU64::new(0),
        }
    }
    fn set_override(&self, path: &str) {
        let path = path.trim();
        let value = (!path.is_empty()).then(|| path.to_owned());
        let changed = {
            let mut slot = self.over.write().unwrap_or_else(|e| e.into_inner());
            let changed = *slot != value;
            *slot = value;
            changed
        };
        if changed {
            self.recheck();
        }
    }
    fn configured_override(&self) -> Option<String> {
        self.over.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn recheck(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    fn cached(&self) -> Option<Option<NodeTools>> {
        let cache = self.cache.try_lock().ok()?;
        cache.as_ref().map(|c| c.tools.clone())
    }
    fn resolve(&self, probe: &dyn Probe) -> Option<NodeTools> {
        let _one_at_a_time = self.resolving.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Cached { tools, at }) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let still_there = match tools {
                Some(t) => probe.is_executable(&t.npx) && probe.is_executable(&t.node),
                None => at.elapsed() < MISS_TTL,
            };
            if still_there {
                return tools.clone();
            }
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let tools = resolve_with(probe, self.configured_override().as_deref());
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if self.generation.load(Ordering::SeqCst) == generation {
            *cache = Some(Cached {
                tools: tools.clone(),
                at: Instant::now(),
            });
        }
        tools
    }
}
static STATE: State = State::new();

/// The path from Settings. A changed value drops the cached result.
pub fn set_override(path: &str) {
    STATE.set_override(path);
}
pub fn configured_override() -> Option<String> {
    STATE.configured_override()
}
/// Forget the result: the next [`resolve`] looks again (the Recheck button). A lookup already
/// running is not remembered when it ends.
pub fn recheck() {
    STATE.recheck();
}
/// What the last lookup found, without starting one: `None` when nothing was looked up yet,
/// `Some(None)` when it found no node. Safe on the UI thread.
pub fn cached() -> Option<Option<NodeTools>> {
    STATE.cached()
}
/// [`resolve_with`] on the real machine, remembered. May run the login shell (up to 5 seconds):
/// call it from a worker.
pub fn resolve() -> Option<NodeTools> {
    STATE.resolve(&RealProbe)
}

/// [`validate_override`] on the real machine.
pub fn validate_real(raw: &str) -> Result<NodeTools, String> {
    validate_override(&RealProbe, raw)
}
