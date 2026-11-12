//! Docker cloud sandbox control through the `sbx` CLI.
//!
//! Every `sbx` call goes through the injectable [`SbxRunner`] so tests never touch the
//! network or the real cloud. Secret values only ever travel over stdin, never in argv.
//!
//! Every call also names the Docker account it runs as ([`SbxRunner::run_as`]; the verb
//! helpers take a runner, and [`as_account`] binds one to an account). `None` is the
//! machine's `sbx login`, today's behavior; an account runs `sbx` with a fresh
//! `DOCKER_ACCESS_TOKEN` in its environment ([`crate::docker_accounts::env_for`]).
//! [`SystemSbx`] is the only place an `sbx` process is spawned.
//!
//! Two rules shape this module:
//!
//! * Every call is `sbx --cloud --no-input ...`: the app only ever talks to cloud
//!   sandboxes and a call must fail rather than wait for an answer nobody can give.
//! * Background code must only ever [`inspect`] / [`state`] / [`ttl`] / [`list`]. Any `ssh`
//!   or `exec` into a stopped sandbox auto-resumes it (`auto_resume: true`) and starts
//!   billing again, so nothing here resumes a sandbox and no helper runs ssh.
use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::cloud_sessions::CloudState;

pub mod account;
#[cfg(test)]
pub(crate) mod test_support;

/// Default sandbox lifetime before `--on-timeout stop` stops it.
pub const DEFAULT_TTL: &str = "2h";
/// Hard server-side ceiling on a sandbox lifetime.
pub const MAX_TTL_SECONDS: u64 = 24 * 60 * 60;
/// The ssh `Host` entry `sbx setup ssh` writes for cloud sandboxes.
pub const SSH_HOST_ALIAS: &str = "sbx_cloud";
/// Longest sandbox name we generate or accept.
pub const NAME_MAX: usize = 63;

const SBX_MISSING: &str = "Docker Sandboxes CLI (sbx) is not installed";
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// `create` pulls the kit and boots a VM (~9 s measured); `rm` waits up to 60 s itself.
const SLOW_CALL_TIMEOUT: Duration = Duration::from_secs(120);
const BRANCH_SLUG_MAX: usize = 30;
const REQUEST_SUFFIX_LEN: usize = 6;
/// `(cpus, memory)` pairs the cloud accepts: micro, small, medium, large, xl.
const SHAPES: [(u8, &str); 5] = [(1, "2g"), (2, "4g"), (4, "8g"), (8, "16g"), (16, "32g")];

/// Captured result of one `sbx` invocation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SbxOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `sbx` with the given arguments. `Err` means sbx could not be started.
pub trait SbxRunner: Send + Sync {
    /// Runs as the machine's `sbx login`.
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<SbxOutput, String>;
    /// Runs as Docker account `account` (an id from [`crate::docker_accounts`]); `None` is
    /// [`Self::run`]. The default ignores the account, so a runner that never talks to the
    /// real `sbx` (every fake) needs no change; [`SystemSbx`] honors it. `Err` is also what
    /// an account that cannot produce a token reports (never a fallback to the machine
    /// login).
    fn run_as(
        &self,
        account: Option<&str>,
        args: &[String],
        stdin: Option<&str>,
    ) -> Result<SbxOutput, String> {
        let _ = account;
        self.run(args, stdin)
    }
}

/// A runner bound to one Docker account: every call of the verb helpers made through it
/// runs as that account. Build it with [`as_account`].
pub struct AsAccount<'a> {
    runner: &'a dyn SbxRunner,
    account: Option<String>,
}

/// `runner` bound to `account` (`None` keeps the machine login).
pub fn as_account<'a>(runner: &'a dyn SbxRunner, account: Option<&str>) -> AsAccount<'a> {
    AsAccount {
        runner,
        account: account.map(str::to_owned),
    }
}

impl SbxRunner for AsAccount<'_> {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<SbxOutput, String> {
        self.runner.run_as(self.account.as_deref(), args, stdin)
    }
    fn run_as(
        &self,
        account: Option<&str>,
        args: &[String],
        stdin: Option<&str>,
    ) -> Result<SbxOutput, String> {
        self.runner.run_as(account, args, stdin)
    }
}

/// Real runner: resolves `sbx` on the login-shell PATH like the other CLIs.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSbx;

impl SystemSbx {
    /// The `sbx` process for `args`. With an account, `DOCKER_ACCESS_TOKEN` is set on the
    /// child's environment (never argv); without one nothing is added.
    fn command(program: &Path, args: &[String], account: Option<&str>) -> Result<Command, String> {
        let mut cmd = Command::new(program);
        cmd.args(with_base_flags(args))
            .env("NO_COLOR", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        crate::scm::shell::apply_gui_env(&mut cmd);
        crate::process::configure_child(&mut cmd);
        // After the GUI env so an inherited DOCKER_ACCESS_TOKEN can not win over the account.
        crate::docker_accounts::apply_env(&mut cmd, account)?;
        Ok(cmd)
    }
}

/// Runs one `sbx` call as `account`. `attempt` spawns the process (it builds the command,
/// so every attempt reads a fresh token from the account cache). When `sbx` says the JWT was
/// rejected, the account's cached JWT is dropped and the call is made once more; a second
/// rejection is the account's token being revoked or expired.
fn run_as_account(
    account: Option<&str>,
    attempt: &mut dyn FnMut() -> Result<SbxOutput, String>,
) -> Result<SbxOutput, String> {
    let rejected = |out: &SbxOutput| {
        !out.success
            && (crate::docker_accounts::is_rejection(&out.stderr)
                || crate::docker_accounts::is_rejection(&out.stdout))
    };
    let out = attempt()?;
    let Some(account) = account.filter(|_| rejected(&out)) else {
        return Ok(out);
    };
    let access = crate::docker_accounts::shared();
    if let Some(access) = &access {
        access.invalidate(account);
    }
    let out = attempt()?;
    if !rejected(&out) {
        return Ok(out);
    }
    Err(match access {
        Some(access) => access.rejected(account),
        None => crate::docker_accounts::DockerAuthError::NotAvailable,
    }
    .to_string())
}

/// `args` with `--cloud --no-input` in front unless the caller already put them there.
fn with_base_flags(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len() + 2);
    for flag in ["--cloud", "--no-input"] {
        if !args.iter().any(|a| a == flag) {
            out.push(flag.to_string());
        }
    }
    out.extend(args.iter().cloned());
    out
}

fn spawn_error(error: std::io::Error) -> String {
    if error.kind() == ErrorKind::NotFound {
        SBX_MISSING.to_string()
    } else {
        error.to_string()
    }
}

/// How long one `sbx` call may take before it is killed.
fn call_timeout(args: &[String]) -> Duration {
    let verb = args
        .iter()
        .map(String::as_str)
        .find(|a| !a.starts_with('-'))
        .unwrap_or("");
    match verb {
        "create" | "rm" | "stop" => SLOW_CALL_TIMEOUT,
        _ => CALL_TIMEOUT,
    }
}

impl SbxRunner for SystemSbx {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<SbxOutput, String> {
        self.run_as(None, args, stdin)
    }
    fn run_as(
        &self,
        account: Option<&str>,
        args: &[String],
        stdin: Option<&str>,
    ) -> Result<SbxOutput, String> {
        let verb = args
            .iter()
            .map(String::as_str)
            .find(|a| !a.starts_with('-'))
            .unwrap_or("");
        let program =
            crate::scm::resolve_gui_binary("sbx").ok_or_else(|| SBX_MISSING.to_string())?;
        run_as_account(account, &mut || {
            run_piped(
                Self::command(&program, args, account)?,
                stdin,
                call_timeout(args),
                verb,
            )
        })
    }
}

/// Runs `cmd` with piped output, killing it when it outlives `timeout`.
fn run_piped(
    mut cmd: Command,
    stdin: Option<&str>,
    timeout: Duration,
    verb: &str,
) -> Result<SbxOutput, String> {
    {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = cmd.spawn().map_err(spawn_error)?;
        let read = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut text = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut text);
                }
                String::from_utf8_lossy(&text).into_owned()
            })
        };
        // Readers and the stdin writer run on their own threads so a large stdin plus a
        // large stdout cannot deadlock, and the deadline below covers all of them.
        let out = read(child.stdout.take().map(|p| Box::new(p) as _));
        let err = read(child.stderr.take().map(|p| Box::new(p) as _));
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let text = text.to_string();
            std::thread::spawn(move || {
                // A closed pipe means sbx exited early; its status reports why.
                let _ = pipe.write_all(text.as_bytes());
            });
        }
        let deadline = std::time::Instant::now() + timeout;
        let status = loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(status) => break status,
                None if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "Docker Sandboxes CLI did not answer within {} seconds (`sbx {}`). Check your network connection.",
                        timeout.as_secs(),
                        verb
                    ));
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        Ok(SbxOutput {
            success: status.success(),
            stdout: out.join().unwrap_or_default(),
            stderr: err.join().unwrap_or_default(),
        })
    }
}

fn args(parts: &[&str]) -> Vec<String> {
    let mut v = vec!["--cloud".to_string(), "--no-input".to_string()];
    v.extend(parts.iter().map(|s| s.to_string()));
    v
}

/// Masks secrets in CLI output so an error can be shown or logged: URL userinfo, bearer
/// tokens, JSON `token`/`password` values, `KEY=value` secrets and known token prefixes
/// anywhere inside a word.
pub fn redact(text: &str) -> String {
    const PREFIXES: [&str; 10] = [
        "github_pat_",
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "glpat-",
        "sk-ant-",
        "dckr_pat_",
        "dckr_oat_",
        "xoxb-",
    ];
    const SECRET_KEYS: [&str; 5] = ["TOKEN", "SECRET", "PASSWORD", "API_KEY", "PASSWD"];
    const MASK: &str = "[redacted]";
    let is_token_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
    let mut out = String::with_capacity(text.len());
    // Pass 1: JSON string values of secret-looking keys, and `Bearer <x>`.
    let mut rest = text;
    while !rest.is_empty() {
        let lower = rest.to_ascii_lowercase();
        let bearer = lower.find("bearer ");
        let json = quoted_secret_key(&lower, &SECRET_KEYS);
        match (bearer, json) {
            (Some(b), j) if j.is_none_or(|(ji, _)| b < ji) => {
                out.push_str(&rest[..b + 7]);
                out.push_str(MASK);
                let after = &rest[b + 7..];
                let end = after
                    .find(|c: char| !is_token_char(c) && c != '=' && c != '+' && c != '/')
                    .unwrap_or(after.len());
                rest = &after[end..];
            }
            (_, Some((ji, klen))) => {
                let after_key = ji + klen;
                let tail = &rest[after_key..];
                let ws = tail.len() - tail.trim_start().len();
                if let Some(v) = tail[ws..].strip_prefix(':') {
                    let vws = v.len() - v.trim_start().len();
                    if let Some(q) = v[vws..].strip_prefix('"') {
                        let mut close = None;
                        let mut escaped = false;
                        for (i, c) in q.char_indices() {
                            if escaped {
                                escaped = false;
                            } else if c == '\\' {
                                escaped = true;
                            } else if c == '"' {
                                close = Some(i);
                                break;
                            }
                        }
                        let close = close.unwrap_or(q.len());
                        out.push_str(&rest[..after_key]);
                        out.push_str(&tail[..ws]);
                        out.push(':');
                        out.push_str(&v[..vws]);
                        out.push('"');
                        out.push_str(MASK);
                        rest = &q[close..];
                        continue;
                    }
                }
                out.push_str(&rest[..after_key]);
                rest = &rest[after_key..];
            }
            _ => {
                out.push_str(rest);
                break;
            }
        }
    }
    // Pass 2: word by word.
    text_words(&out, |word| {
        let mut w = word.to_string();
        // URL userinfo: scheme://user:secret@host
        if let Some(scheme) = w.find("://") {
            let host_start = scheme + 3;
            let end = w[host_start..]
                .find('/')
                .map_or(w.len(), |i| host_start + i);
            if let Some(at) = w[host_start..end].rfind('@') {
                w.replace_range(host_start..host_start + at, MASK);
            }
        }
        if let Some((key, value)) = w.split_once('=') {
            let key = key.to_ascii_uppercase();
            if !value.is_empty() && SECRET_KEYS.iter().any(|k| key.contains(k)) {
                return MASK.to_string();
            }
        }
        for prefix in PREFIXES {
            while let Some(i) = w.find(prefix) {
                let end = w[i..]
                    .find(|c: char| !is_token_char(c))
                    .map_or(w.len(), |e| i + e);
                w.replace_range(i..end, MASK);
            }
        }
        mask_jwts(&mut w);
        w
    })
}

/// Mask JWT-shaped runs inside `word`: three dot-separated base64url parts, the first
/// starting with `eyJ` (a JSON header).
fn mask_jwts(word: &mut String) {
    let is_part = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-');
    let mut from = 0;
    while let Some(found) = word[from..].find("eyJ") {
        let start = from + found;
        let end = word[start..]
            .find(|c: char| !is_part(c) && c != '.')
            .map_or(word.len(), |e| start + e);
        let run = word[start..end].trim_end_matches('.');
        let parts: Vec<&str> = run.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|p| !p.is_empty()) {
            word.replace_range(start..start + run.len(), "[redacted]");
            from = start + "[redacted]".len();
        } else {
            from = start + 3;
        }
    }
}

/// Start/length of the first `"<key containing a secret word>"` in `lower`.
fn quoted_secret_key(lower: &str, words: &[&str]) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(open) = lower[from..].find('"') {
        let open = from + open;
        let close = lower[open + 1..].find('"')?;
        let key = &lower[open + 1..open + 1 + close];
        let squashed = key.replace(['_', '-'], "");
        if !key.is_empty()
            && key.len() < 40
            && words
                .iter()
                .any(|w| squashed.contains(&w.to_ascii_lowercase().replace('_', "")))
        {
            return Some((open, close + 2));
        }
        from = open + 1 + close + 1;
    }
    None
}

/// Applies `f` to each whitespace-separated word, keeping the whitespace.
fn text_words(text: &str, f: impl Fn(&str) -> String) -> String {
    text.split_inclusive(char::is_whitespace)
        .map(|chunk| {
            let word = chunk.trim_end();
            format!("{}{}", f(word), &chunk[word.len()..])
        })
        .collect()
}

/// Error text of a failed call: stderr, else stdout, redacted.
fn failure(out: &SbxOutput, what: &str) -> String {
    let text = if out.stderr.trim().is_empty() {
        out.stdout.trim()
    } else {
        out.stderr.trim()
    };
    if text.is_empty() {
        format!("`sbx {what}` failed")
    } else {
        redact(text)
    }
}

fn run_ok(runner: &dyn SbxRunner, argv: Vec<String>, what: &str) -> Result<SbxOutput, String> {
    let out = runner.run(&argv, None)?;
    if out.success {
        Ok(out)
    } else {
        Err(failure(&out, what))
    }
}

/// True when `error` is exactly the CLI's "this sandbox does not exist" answer for `reference`
/// (`error: sandbox 'sbx_x' not found`). Other "not found" texts (runtime, session, kit,
/// template) must never read as a deleted sandbox: a false positive makes the app forget a
/// sandbox that is still billing. Only use it on `inspect` / `ttl` results.
pub fn is_not_found_error(error: &str, reference: &str) -> bool {
    let wanted = format!("sandbox '{reference}' not found");
    error.lines().any(|line| {
        let line = line.trim();
        let line = line.strip_prefix("error:").map_or(line, str::trim);
        line.eq_ignore_ascii_case(&wanted)
    })
}

/// True when [`create`] failed because the sandbox name is taken.
pub fn is_already_exists_error(error: &str) -> bool {
    error.to_ascii_lowercase().contains("already_exists")
}

// ---------------------------------------------------------------- validation

/// A sandbox name: >= 2 chars, starts with a letter or digit, letters/digits/hyphens only.
pub fn validate_sandbox_name(name: &str) -> Result<(), String> {
    if name.len() < 2 {
        return Err("Sandbox name must have at least 2 characters".into());
    }
    if name.len() > NAME_MAX {
        return Err(format!(
            "Sandbox name must have at most {NAME_MAX} characters"
        ));
    }
    if !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return Err("Sandbox name must start with a letter or a number".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Sandbox name may only contain letters, numbers and hyphens".into());
    }
    Ok(())
}

/// A sandbox id: `sbx_` followed by lowercase alphanumerics.
pub fn validate_sandbox_id(id: &str) -> Result<(), String> {
    match id.strip_prefix("sbx_") {
        Some(rest)
            if !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => Err(format!("Not a sandbox id: {id}")),
    }
}

/// Parses a Go-style duration made of `h`/`m`/`s` parts (`30m`, `2h`, `1h30m`), in seconds.
fn parse_duration_seconds(text: &str) -> Result<u64, String> {
    let bad = || format!("Invalid duration `{text}` (use e.g. 30m, 2h, 1h30m)");
    if text.is_empty() {
        return Err(bad());
    }
    let mut total = 0u64;
    let mut digits = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit = match c.to_ascii_lowercase() {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return Err(bad()),
        };
        let n: u64 = digits.parse().map_err(|_| bad())?;
        digits.clear();
        total = total.saturating_add(n.saturating_mul(unit));
    }
    if !digits.is_empty() || total == 0 {
        return Err(bad());
    }
    Ok(total)
}

/// Validates a create-time `--ttl` (positive, at most the 24 h server ceiling).
pub fn validate_ttl(ttl: &str) -> Result<(), String> {
    let seconds = parse_duration_seconds(ttl)?;
    if seconds > MAX_TTL_SECONDS {
        return Err("TTL cannot exceed 24h".into());
    }
    Ok(())
}

/// Requested sandbox size and lifetime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compute {
    pub cpus: u8,
    /// Binary units as the CLI takes them, e.g. `4g`.
    pub memory: String,
    pub ttl: String,
    /// `linux/amd64` or `linux/arm64`; `None` lets the server choose.
    pub platform: Option<String>,
}

impl Default for Compute {
    fn default() -> Self {
        Self {
            cpus: 2,
            memory: "4g".into(),
            ttl: DEFAULT_TTL.into(),
            platform: None,
        }
    }
}

impl Compute {
    /// Checks the cpus/memory pair is a billable shape and ttl/platform are well formed.
    pub fn validate(&self) -> Result<(), String> {
        let memory = self.memory.to_ascii_lowercase();
        if !SHAPES.iter().any(|(c, m)| *c == self.cpus && *m == memory) {
            let shapes = SHAPES
                .iter()
                .map(|(c, m)| format!("{c} CPU / {m}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "{} CPUs with {} memory is not a billable shape. Choose one of: {shapes}",
                self.cpus, self.memory
            ));
        }
        validate_ttl(&self.ttl)?;
        if let Some(p) = &self.platform {
            if p != "linux/amd64" && p != "linux/arm64" {
                return Err(format!("Unsupported platform `{p}`"));
            }
        }
        Ok(())
    }
}

/// `monoptah-<branch-slug>-<6 chars of the request id>`; always a valid sandbox name.
pub fn sandbox_name_for(branch: &str, request_id: &str) -> String {
    let mut slug = String::new();
    for c in branch.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let mut slug: String = slug.chars().take(BRANCH_SLUG_MAX).collect();
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug = "session".into();
    }
    let mut suffix: String = request_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(REQUEST_SUFFIX_LEN)
        .collect();
    while suffix.len() < REQUEST_SUFFIX_LEN {
        suffix.push('0');
    }
    format!("monoptah-{slug}-{suffix}")
}

// --------------------------------------------------------------------- types

/// A JSON string, number or bool read as text; `None` for null or anything else.
fn lenient_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

/// One sandbox as `sbx --cloud inspect <id> --format json` / `ls --format json` report it.
/// Unknown fields are ignored and missing ones default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SandboxInfo {
    pub id: String,
    pub name: String,
    pub agent: String,
    pub state: String,
    pub image: String,
    pub cpus: u32,
    pub memory_mib: u64,
    pub on_timeout: String,
    pub auto_resume: bool,
    #[serde(deserialize_with = "lenient_text")]
    pub created_at: Option<String>,
    #[serde(deserialize_with = "lenient_text")]
    pub expires_at: Option<String>,
    pub ttl_paused: bool,
}

/// `sbx --cloud ttl <id> --json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TtlInfo {
    pub expires_in_seconds: Option<i64>,
    pub max_remaining_seconds: Option<i64>,
    pub ttl_paused: bool,
}

// ----------------------------------------------------------------- lifecycle

/// Why [`create`] failed. A failed call may still have created a sandbox, so the caller
/// must act on `id` (clean up or adopt it) and, when `id` is `None` after a timeout or a
/// start failure, look the name up with [`find_by_name`] before retrying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateError {
    /// An `sbx_...` id the CLI printed before failing, if any.
    pub id: Option<String>,
    /// The CLI's own (redacted) error text.
    pub message: String,
    /// The name is already taken; resolve it with [`find_by_name`].
    pub already_exists: bool,
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for CreateError {
    fn from(message: String) -> Self {
        Self {
            id: None,
            already_exists: is_already_exists_error(&message),
            message,
        }
    }
}

/// Creates a `shell` sandbox and returns its id. `--ttl` and `--on-timeout stop` are
/// always explicit so a forgotten sandbox stops (and keeps its state) instead of running on.
pub fn create(
    runner: &dyn SbxRunner,
    name: &str,
    compute: &Compute,
) -> Result<String, CreateError> {
    validate_sandbox_name(name)?;
    compute.validate()?;
    let cpus = compute.cpus.to_string();
    let mut argv = args(&[
        "create",
        "--no-progress",
        "--name",
        name,
        "--cpus",
        &cpus,
        "--memory",
        &compute.memory.to_ascii_lowercase(),
        "--ttl",
        &compute.ttl,
        "--on-timeout",
        "stop",
    ]);
    if let Some(platform) = &compute.platform {
        argv.push("--platform".into());
        argv.push(platform.clone());
    }
    argv.push("shell".into());
    let out = runner.run(&argv, None)?;
    let id = parse_created_id(&out.stdout).or_else(|| parse_created_id(&out.stderr));
    if !out.success {
        let message = failure(&out, "create");
        return Err(CreateError {
            id,
            already_exists: is_already_exists_error(&message),
            message,
        });
    }
    id.ok_or_else(|| CreateError {
        id: None,
        message: "Docker Sandboxes did not report the new sandbox id".into(),
        already_exists: false,
    })
}

/// Removes ANSI CSI escape sequences (colours, cursor moves, line erases).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&n) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The `sbx_...` id on its own line of `create` output (progress and spinner lines, ANSI
/// codes and carriage returns are mixed in).
pub fn parse_created_id(output: &str) -> Option<String> {
    strip_ansi(output)
        .split(['\n', '\r'])
        .map(str::trim)
        .find(|line| validate_sandbox_id(line).is_ok())
        .map(str::to_string)
}

/// Finds the one `shell` sandbox called exactly `name` (cloud names are not unique handles).
/// Use it after a `create` that timed out or reported `already_exists`. More than one match
/// is an error listing the ids rather than a guess.
pub fn find_by_name(runner: &dyn SbxRunner, name: &str) -> Result<Option<SandboxInfo>, String> {
    validate_sandbox_name(name)?;
    let mut found: Vec<SandboxInfo> = list(runner)?
        .into_iter()
        .filter(|s| s.name == name && s.agent == "shell")
        .collect();
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => Err(format!(
            "More than one sandbox is named {name}: {}",
            found
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Reads one sandbox. Read-only; never resumes it.
pub fn inspect(runner: &dyn SbxRunner, id: &str) -> Result<SandboxInfo, String> {
    validate_sandbox_id(id)?;
    let out = run_ok(
        runner,
        args(&["inspect", id, "--format", "json"]),
        "inspect",
    )?;
    parse_inspect(&out.stdout)
}

fn parse_inspect(json: &str) -> Result<SandboxInfo, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Unreadable sandbox details: {e}"))?;
    let value = match value {
        serde_json::Value::Array(mut items) if !items.is_empty() => items.remove(0),
        other => other,
    };
    serde_json::from_value(value).map_err(|e| format!("Unreadable sandbox details: {e}"))
}

/// All sandboxes of the account.
pub fn list(runner: &dyn SbxRunner) -> Result<Vec<SandboxInfo>, String> {
    let out = run_ok(runner, args(&["ls", "--format", "json"]), "ls")?;
    parse_list(&out.stdout)
}

fn parse_list(json: &str) -> Result<Vec<SandboxInfo>, String> {
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let bad = |e: serde_json::Error| format!("Unreadable sandbox list: {e}");
    let value: serde_json::Value = serde_json::from_str(json).map_err(bad)?;
    let items = match value {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Array(_) => value,
        // An object wrapping the array, e.g. {"sandboxes": [...]}.
        serde_json::Value::Object(map) => match map.into_iter().find(|(_, v)| v.is_array()) {
            Some((_, array)) => array,
            None => return Ok(Vec::new()),
        },
        _ => return Err("Unreadable sandbox list: unexpected JSON".into()),
    };
    serde_json::from_value(items).map_err(bad)
}

/// Raw lifecycle state (`running`, `stopping`, `stopped`, ...), from `inspect`.
pub fn state(runner: &dyn SbxRunner, id: &str) -> Result<String, String> {
    Ok(inspect(runner, id)?.state)
}

/// Asks the cloud to stop the sandbox; returns once accepted (the state then passes
/// through `stopping`, about two minutes, before `stopped`).
pub fn stop(runner: &dyn SbxRunner, id: &str) -> Result<(), String> {
    validate_sandbox_id(id)?;
    run_ok(runner, args(&["stop", id]), "stop").map(|_| ())
}

/// Deletes the sandbox (`rm --force <id>`). `--force` is required: without it `rm` asks for
/// confirmation, which fails under `--no-input`. Only call this after the app's own user
/// confirmation.
pub fn delete(runner: &dyn SbxRunner, id: &str) -> Result<(), String> {
    validate_sandbox_id(id)?;
    run_ok(runner, args(&["rm", "--force", id]), "rm").map(|_| ())
}

/// Remaining lifetime of the sandbox.
pub fn ttl(runner: &dyn SbxRunner, id: &str) -> Result<TtlInfo, String> {
    validate_sandbox_id(id)?;
    let out = run_ok(runner, args(&["ttl", id, "--json"]), "ttl")?;
    serde_json::from_str(&out.stdout).map_err(|e| format!("Unreadable TTL details: {e}"))
}

/// Extends the lifetime by `duration` (`2h`, `30m`). Only for an explicit user action:
/// the server caps the total at 24 h from creation and never shortens it.
pub fn extend_ttl(runner: &dyn SbxRunner, id: &str, duration: &str) -> Result<(), String> {
    validate_sandbox_id(id)?;
    parse_duration_seconds(duration)?;
    let plus = format!("+{duration}");
    run_ok(runner, args(&["ttl", &plus, id]), "ttl").map(|_| ())
}

// ------------------------------------------------------------- state mapping

/// Maps a raw sandbox state to the shared displayable state.
pub fn display_state(raw: &str) -> CloudState {
    match raw.trim().to_ascii_lowercase().as_str() {
        "running" => CloudState::Available,
        "creating" | "starting" | "resuming" | "pending" | "provisioning" | "restarting" => {
            CloudState::Starting
        }
        "stopping" | "suspending" => CloudState::Stopping,
        "stopped" | "suspended" => CloudState::Stopped,
        "failed" | "error" | "deleting" | "deleted" | "terminated" => CloudState::Unavailable,
        _ => CloudState::Unknown,
    }
}

/// True for the settled stopped state (resumable, TTL paused).
pub fn is_stopped(raw: &str) -> bool {
    display_state(raw) == CloudState::Stopped
}

/// True when an `inspect` / `ttl` error says sandbox `id` does not exist (see
/// [`is_not_found_error`]).
pub fn is_deleted(error: &str, id: &str) -> bool {
    is_not_found_error(error, id)
}

// ----------------------------------------------------------------------- ssh

/// `<id>@sbx_cloud`: the ssh destination of a sandbox (`%r` of the alias is the id).
pub fn ssh_destination(id: &str) -> String {
    format!("{id}@{SSH_HOST_ALIAS}")
}

/// True when `~/.ssh/config` (or an include of it, always counting
/// `~/.ssh/sandboxes/config`) has a `Host sbx_cloud` entry. Read-only.
pub fn ssh_alias_configured(home: &Path) -> bool {
    let ssh_dir = home.join(".ssh");
    let mut seen = HashSet::new();
    let mut files = vec![
        ssh_dir.join("config"),
        ssh_dir.join("sandboxes").join("config"),
    ];
    let mut depth = 0;
    while let Some(path) = files.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let mut words = line.split_whitespace();
            match words.next().map(str::to_ascii_lowercase).as_deref() {
                Some("host") if words.any(|w| w == SSH_HOST_ALIAS) => return true,
                Some("include") if depth < 16 => {
                    for w in words {
                        files.push(resolve_include(home, &ssh_dir, w.trim_matches('"')));
                        depth += 1;
                    }
                }
                _ => {}
            }
        }
    }
    false
}

fn resolve_include(home: &Path, ssh_dir: &Path, word: &str) -> PathBuf {
    match word.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if Path::new(word).is_absolute() => PathBuf::from(word),
        None => ssh_dir.join(word),
    }
}

// ------------------------------------------------------------------ diagnose

/// Whether this machine can create and open cloud sandboxes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prerequisites {
    SbxMissing,
    /// Carries the CLI's own error text.
    NotLoggedIn(String),
    SshAliasMissing,
    /// sbx answered with something other than a login problem (network, service).
    Unreachable(String),
    /// The project's Docker account could not be used: the text says why and where to fix
    /// it (Settings > Cloud). There is no command to run and `sbx login` is not the advice.
    DockerAccount(String),
    Ready,
}

impl Prerequisites {
    /// The command the user should run to fix this, if there is one.
    pub fn fix_command(&self) -> Option<&'static str> {
        match self {
            Self::NotLoggedIn(_) => Some("sbx login"),
            Self::SshAliasMissing => Some("sbx --cloud setup ssh"),
            _ => None,
        }
    }
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
    /// One line for the user, naming the command to run.
    pub fn message(&self) -> String {
        match self {
            Self::SbxMissing => {
                "Docker Sandboxes CLI (sbx) is not installed. Install it from docs.docker.com/ai/sandboxes.".into()
            }
            Self::NotLoggedIn(why) => format!("Sign in to Docker first: run `sbx login`. ({why})"),
            Self::SshAliasMissing => {
                "SSH is not set up for cloud sandboxes: run `sbx --cloud setup ssh`.".into()
            }
            Self::Unreachable(why) => format!("Docker Sandboxes is not reachable: {why}"),
            Self::DockerAccount(why) => why.clone(),
            Self::Ready => "Ready".into(),
        }
    }
}

/// Checks the prerequisites in order: sbx installed, signed in, ssh alias present.
///
/// `sbx diagnose` is not used as the probe: it inspects the local sandbox daemon, which
/// reports `fail` on machines that only use cloud sandboxes (verified with sbx 0.51.0
/// where cloud calls work with the daemon down). A cheap authenticated read, `ls`, proves
/// both the install and the login.
pub fn diagnose(runner: &dyn SbxRunner, home: &Path) -> Prerequisites {
    diagnose_as(runner, home, None)
}

/// [`diagnose`] for a project that names Docker account `account`: the probe runs as that
/// account, and a problem with the account (unknown, revoked, no Cloud Sandboxes access,
/// rejected by sbx) is [`Prerequisites::DockerAccount`] with the account's own advice
/// instead of `sbx login`. `None` is [`diagnose`].
pub fn diagnose_as(runner: &dyn SbxRunner, home: &Path, account: Option<&str>) -> Prerequisites {
    if account.is_some() {
        // A token problem is reported as such before sbx is even asked.
        if let Err(error) = crate::docker_accounts::env_for(account) {
            return Prerequisites::DockerAccount(error.to_string());
        }
    }
    match runner.run_as(account, &args(&["ls", "--format", "json"]), None) {
        Err(error) if error.contains("not installed") => return Prerequisites::SbxMissing,
        Err(error) if account.is_some() && is_account_text(&error) => {
            return Prerequisites::DockerAccount(error)
        }
        Err(error) => return Prerequisites::Unreachable(redact(&error)),
        Ok(out) if !out.success => {
            let error = failure(&out, "ls");
            return if account.is_some() && (is_account_rejection(&out) || looks_logged_out(&error))
            {
                // The CLI's own text may advise `sbx login`, which is the wrong fix here.
                Prerequisites::DockerAccount(
                    "Docker Sandboxes did not accept this Docker account's token. Replace it in Settings > Cloud."
                        .into(),
                )
            } else if looks_logged_out(&error) {
                Prerequisites::NotLoggedIn(error)
            } else {
                Prerequisites::Unreachable(error)
            };
        }
        Ok(_) => {}
    }
    if ssh_alias_configured(home) {
        Prerequisites::Ready
    } else {
        Prerequisites::SshAliasMissing
    }
}

/// True when `sbx` reported the account's JWT as not accepted.
fn is_account_rejection(out: &SbxOutput) -> bool {
    crate::docker_accounts::is_rejection(&out.stderr)
        || crate::docker_accounts::is_rejection(&out.stdout)
}

/// True when `text` is the user-facing text of a Docker account failure (an error of
/// [`crate::docker_accounts::DockerAuthError`] that tells where to fix it).
pub fn is_account_text(text: &str) -> bool {
    text.contains("in Settings > Cloud.") || text.contains("Choose another in Cloud settings.")
}

fn looks_logged_out(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "sbx login",
        "not logged in",
        "not signed in",
        "sign in",
        "sign-in",
        "session expired",
        "unauthenticated",
        "unauthorized",
        "authentication required",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests;
