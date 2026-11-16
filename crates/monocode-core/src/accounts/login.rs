use super::Profile;
use crate::{
    process,
    provider::{self, Kind},
};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthAction {
    SignIn,
    SignOut,
}
pub fn arguments(kind: Kind, action: AuthAction) -> Option<&'static [&'static str]> {
    match (kind, action) {
        (Kind::Claude | Kind::Devin, AuthAction::SignIn) => Some(&["auth", "login"]),
        (Kind::Claude | Kind::Devin, AuthAction::SignOut) => Some(&["auth", "logout"]),
        (Kind::Codex | Kind::Cursor, AuthAction::SignIn) => Some(&["login"]),
        (Kind::Codex | Kind::Cursor, AuthAction::SignOut) => Some(&["logout"]),
        (Kind::Grok, AuthAction::SignIn) => Some(&["login", "--oauth"]),
        (Kind::Fx, AuthAction::SignIn) => Some(&["login", "vercel"]),
        _ => None,
    }
}
/// Whether this provider's sign-in CLI reads a pasted verification code from
/// its stdin while it also waits for the browser callback. Only Claude Code is
/// known to: its `auth login` prints "Paste code here if prompted" and reads
/// `code#state` lines. Other CLIs keep a closed stdin until proven otherwise.
pub fn accepts_pasted_code(kind: Kind) -> bool {
    kind == Kind::Claude
}
/// How a browser sign-in reaches the user. `Browser` is the CLI's own flow: it
/// opens the browser on a localhost callback. `Code` keeps the same CLI process
/// but stops it opening anything: the app opens the CLI's hosted code page
/// (see [`manual_sign_in_url`]) and the user pastes the code it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignInMode {
    #[default]
    Browser,
    Code,
}
/// The hosted code page URL, or `None` when `raw` is anything else. The CLI
/// prints two authorize URLs with the same state and PKCE challenge: this one
/// redirects to an https page that shows a code, the other to a localhost
/// callback that never shows one. Only the former may be offered or opened.
pub fn manual_sign_in_url(raw: &str) -> Option<String> {
    use url::{Host, Url};
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    // The hosted code page is requested with `code=true`; exactly one redirect.
    if !url.query_pairs().any(|(k, v)| k == "code" && v == "true") {
        return None;
    }
    let mut redirects = url.query_pairs().filter(|(key, _)| key == "redirect_uri");
    let (_, redirect) = redirects.next()?;
    if redirects.next().is_some() {
        return None;
    }
    let redirect = Url::parse(&redirect).ok()?;
    if redirect.scheme() != "https" {
        return None;
    }
    let local = match redirect.host()? {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(ip) => ip.is_loopback() || ip.is_unspecified(),
        Host::Ipv6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
        }
    };
    (!local).then(|| raw.to_owned())
}
/// In code mode, stop the CLI from opening its own (localhost) browser page.
/// The CLI opens pages by running `$BROWSER <url>` and ignores the result, so
/// the no-op `true` makes that open do nothing. Only applied on unix: how the Windows CLI honours
/// `BROWSER` is not verified, so there it may also open the localhost page,
/// which is harmless because either path can finish the same sign-in.
fn apply_mode(command: &mut Command, kind: Kind, mode: SignInMode) {
    if cfg!(unix) && kind == Kind::Claude && mode == SignInMode::Code {
        command.env("BROWSER", "true");
    }
}
/// Longest pasted code accepted. Real codes are well under 200 bytes.
const MAX_CODE_LEN: usize = 2048;
/// Progress of a running sign-in that the UI may want to show. Values here are
/// secrets or near secrets: the type has no printing `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub enum AuthProgress {
    /// The CLI is asking for a pasted code. `url` is the sign-in page it printed.
    CodePrompt { url: Option<String> },
    /// The CLI (or validation) refused a pasted code but still waits for one.
    CodeRejected(String),
}
impl std::fmt::Debug for AuthProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CodePrompt { .. } => "AuthProgress::CodePrompt",
            Self::CodeRejected(_) => "AuthProgress::CodeRejected",
        })
    }
}
/// The two-way channel between a running sign-in and its UI.
pub struct AuthIo {
    /// Codes the user pasted, delivered to the CLI's stdin.
    pub codes: mpsc::Receiver<String>,
    /// Called from background threads when the CLI shows a prompt or rejects a code.
    pub progress: Arc<dyn Fn(AuthProgress) + Send + Sync>,
}
/// Validate a pasted `code#state` and return the exact line to send.
pub fn validate_code(raw: &str) -> Result<String, String> {
    let code = raw.trim();
    if code.is_empty() {
        return Err("Paste the code shown in your browser.".into());
    }
    if code.len() > MAX_CODE_LEN {
        return Err("That code is too long. Copy only the code shown in your browser.".into());
    }
    if code.chars().any(char::is_control) {
        return Err("That code has line breaks or hidden characters. Copy it again.".into());
    }
    match code.split_once('#') {
        Some((a, b)) if !a.is_empty() && !b.is_empty() && !b.contains('#') => Ok(code.to_owned()),
        _ => Err(
            "That code looks incomplete. Copy the whole code, including the part after #.".into(),
        ),
    }
}
fn zeroize(value: String) {
    let mut bytes = value.into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}
/// Remove every submitted code, and each half of it, from `text`.
fn redact(text: &str, secrets: &[String]) -> String {
    // Longest first, so the whole code goes before any of its halves.
    let mut ordered: Vec<&String> = secrets.iter().filter(|s| !s.is_empty()).collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut text = text.to_owned();
    for secret in ordered {
        text = text.replace(secret.as_str(), "[code]");
    }
    text
}
/// Reduce terminal output to plain text. OSC 8 hyperlinks keep their target
/// (surrounded by spaces) because the visible text may not be the URL.
fn strip_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(']') => {
                let mut body = String::new();
                while let Some(n) = chars.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                    body.push(n);
                }
                if let Some(uri) = body.strip_prefix("8;").and_then(|b| b.split_once(';')) {
                    if !uri.1.is_empty() {
                        out.push(' ');
                        out.push_str(uri.1);
                        out.push(' ');
                    }
                }
            }
            Some('[') => {
                for n in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
/// Finds the code prompt and sign-in URL in the CLI's stdout, which arrives in
/// arbitrary pieces. Keeps a bounded tail until the prompt is seen, then nothing.
#[derive(Default)]
struct PromptScanner {
    buffer: Vec<u8>,
    done: bool,
}
impl PromptScanner {
    fn feed(&mut self, chunk: &[u8]) -> Option<AuthProgress> {
        if self.done {
            return None;
        }
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > 16_384 {
            self.buffer.drain(..self.buffer.len() - 16_384);
        }
        let text = strip_escapes(&String::from_utf8_lossy(&self.buffer));
        if !text.contains("Paste code here") {
            return None;
        }
        self.done = true;
        self.buffer = Vec::new();
        Some(AuthProgress::CodePrompt {
            url: sign_in_url(&text),
        })
    }
}
/// The sign-in page: the URL on the "visit:" or "Sign in at:" line, else the
/// last https URL printed before the prompt.
fn sign_in_url(text: &str) -> Option<String> {
    let link = regex::Regex::new(r"https://[^\s\x00-\x1f]+").unwrap();
    let clean = |raw: &str| -> String {
        raw.trim_end_matches(['.', ',', ')', ';'])
            .chars()
            .take(4096)
            .collect()
    };
    let before = &text[..text.find("Paste code here").unwrap_or(text.len())];
    before
        .lines()
        .rev()
        .filter(|line| line.contains("visit:") || line.contains("Sign in at:"))
        .find_map(|line| link.find(line))
        .or_else(|| link.find_iter(before).last())
        .map(|m| clean(m.as_str()))
}
/// Counts the CLI's "Invalid code" lines, which it prints to stderr while it
/// keeps waiting for another code.
#[derive(Default)]
struct RejectScanner {
    line: Vec<u8>,
}
impl RejectScanner {
    fn feed(&mut self, chunk: &[u8]) -> usize {
        let mut count = 0;
        for &byte in chunk {
            if byte == b'\n' {
                if String::from_utf8_lossy(&self.line).contains("Invalid code") {
                    count += 1;
                }
                self.line.clear();
            } else if self.line.len() < 512 {
                self.line.push(byte);
            }
        }
        count
    }
}
const REJECTED: &str =
    "The CLI says that code is incomplete. Copy the whole code, including the part after #.";
const NOT_ACCEPTED: &str =
    "That code was not accepted or has expired. Start sign-in again to get a new code.";
/// The CLI opens its browser and owns credential writes. Never relay its URLs,
/// OAuth codes or tokens to logs, clipboard, or the native account metadata.
pub fn authenticate(
    kind: Kind,
    profile: &Profile,
    action: AuthAction,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    authenticate_with(kind, profile, action, cancel, None, SignInMode::Browser)
}
/// Like [`authenticate`], and for providers that accept one, also lets the user
/// paste a verification code (see [`accepts_pasted_code`]).
pub fn authenticate_with(
    kind: Kind,
    profile: &Profile,
    action: AuthAction,
    cancel: Arc<AtomicBool>,
    io: Option<AuthIo>,
    mode: SignInMode,
) -> Result<(), String> {
    // A cancelled attempt must never start (or touch the credentials of) anything.
    if cancel.load(Ordering::Relaxed) {
        return Err("Sign-in cancelled".into());
    }
    if kind == Kind::Claude && profile.uses_claude_token()? {
        if action == AuthAction::SignOut {
            profile.remove_claude_token()?;
        } else {
            return Err(
                "Replace or remove the saved token or API key in Accounts before using browser login.".into(),
            );
        }
    }
    let args = arguments(kind, action)
        .ok_or("This provider does not offer a single browser sign-in flow")?;
    let binary =
        provider::resolve(kind).ok_or_else(|| format!("{} CLI not installed", kind.label()))?;
    let mut command = Command::new(binary);
    crate::scm::shell::apply_gui_env(&mut command);
    profile.apply(kind, &mut command)?;
    command.args(args);
    if let Some(home) = super::home() {
        command.current_dir(home);
    }
    let io = code_io(kind, action, io);
    // Without the code channel nothing could finish the sign-in, so the CLI
    // keeps its own browser flow.
    if io.is_some() {
        apply_mode(&mut command, kind, mode);
    }
    let timeout = Duration::from_secs(if action == AuthAction::SignIn {
        600
    } else {
        30
    });
    run(command, kind, profile, timeout, cancel, io)
}
/// Keep the code channel only where the CLI can use it.
fn code_io(kind: Kind, action: AuthAction, io: Option<AuthIo>) -> Option<AuthIo> {
    io.filter(|_| action == AuthAction::SignIn && accepts_pasted_code(kind))
}
fn run(
    mut command: Command,
    kind: Kind,
    profile: &Profile,
    timeout: Duration,
    cancel: Arc<AtomicBool>,
    io: Option<AuthIo>,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Sign-in cancelled".into());
    }
    command.stderr(Stdio::piped());
    if io.is_some() {
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
    } else {
        command.stdin(Stdio::null()).stdout(Stdio::null());
    }
    process::isolate(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start {} sign-in: {e}", kind.label()))?;
    let _registration = process::register(child.id());
    let _account_process = super::track(child.id(), kind, profile);
    let mut stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    let progress = io.as_ref().map(|io| io.progress.clone());
    let rejects = progress.clone();
    let reader_done = spawn_bounded(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        let mut scanner = RejectScanner::default();
        // Keep draining the pipe even after the bounded diagnostic tail fills.
        while let Ok(n) = stderr.read(&mut buffer) {
            if n == 0 {
                break;
            }
            if let Some(progress) = &rejects {
                for _ in 0..scanner.feed(&buffer[..n]) {
                    progress(AuthProgress::CodeRejected(REJECTED.into()));
                }
            }
            bytes.extend_from_slice(&buffer[..n]);
            if bytes.len() > 65_536 {
                bytes.drain(..bytes.len() - 65_536);
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    let out_done = child
        .stdout
        .take()
        .zip(progress.clone())
        .map(|(mut out, progress)| {
            spawn_bounded(move || {
                let mut scanner = PromptScanner::default();
                let mut buffer = [0; 4096];
                // Never stop reading: a full pipe would block the CLI.
                while let Ok(n) = out.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    if let Some(event) = scanner.feed(&buffer[..n]) {
                        progress(event);
                    }
                }
            })
        });
    let mut stdin = child.stdin.take();
    let codes = io.map(|io| io.codes);
    let mut submitted: Vec<String> = Vec::new();
    let deadline = Instant::now() + timeout;
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            break Err("Sign-in cancelled".into());
        }
        if Instant::now() >= deadline {
            break Err(format!(
                "{} sign-in timed out. Please try again.",
                kind.label()
            ));
        }
        if let (Some(codes), Some(pipe), Some(progress)) = (&codes, &mut stdin, &progress) {
            while let Ok(raw) = codes.try_recv() {
                match validate_code(&raw) {
                    Ok(code) => {
                        submitted.extend(
                            code.split('#')
                                .map(str::to_owned)
                                .chain(std::iter::once(code.clone())),
                        );
                        let line = format!("{code}\n");
                        let sent = pipe.write_all(line.as_bytes()).and_then(|_| pipe.flush());
                        zeroize(line);
                        zeroize(raw);
                        zeroize(code);
                        let _ = sent;
                    }
                    Err(message) => {
                        zeroize(raw);
                        progress(AuthProgress::CodeRejected(message));
                    }
                }
            }
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(_)) => {
                let detail = rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default();
                let message = safe_detail(&redact(&detail, &submitted));
                break Err(if !submitted.is_empty() {
                    if message.is_empty() {
                        NOT_ACCEPTED.into()
                    } else {
                        format!("{NOT_ACCEPTED} ({message})")
                    }
                } else if message.is_empty() {
                    format!("{} sign-in exited unexpectedly", kind.label())
                } else {
                    message
                });
            }
            Err(e) => break Err(format!("Could not complete sign-in: {e}")),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
        }
    };
    drop(stdin);
    process::terminate(&mut child);
    drop(rx);
    // A detached grandchild may keep a pipe open: never wait on it for long.
    // Late progress is ignored by the UI, which matches events to the attempt.
    let _ = reader_done.recv_timeout(READER_GRACE);
    if let Some(done) = out_done {
        let _ = done.recv_timeout(READER_GRACE);
    }
    submitted.into_iter().for_each(zeroize);
    result
}
const READER_GRACE: Duration = Duration::from_secs(2);
/// Run `work` on a thread and return a channel that closes when it ends.
fn spawn_bounded(work: impl FnOnce() + Send + 'static) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        work();
        drop(tx);
    });
    rx
}
fn safe_detail(value: &str) -> String {
    let last = value
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or("");
    let urls = regex::Regex::new(r"(?i)https?://\S+").unwrap();
    urls.replace_all(last, "sign-in link")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}
pub(crate) fn cli_output(kind: Kind, args: &[&str], profile: &Profile) -> Result<String, String> {
    let binary =
        provider::resolve(kind).ok_or_else(|| format!("{} CLI not found", kind.label()))?;
    let mut command = Command::new(binary);
    crate::scm::shell::apply_gui_env(&mut command);
    profile.apply(kind, &mut command)?;
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    process::isolate(&mut command);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let _registration = process::register(child.id());
    let _account_process = super::track(child.id(), kind, profile);
    let output = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output
            .take(1_048_577)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let result = rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| "Provider request timed out".to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))
        .and_then(|bytes| {
            if bytes.len() > 1_048_576 {
                Err("Provider output exceeded its limit".into())
            } else {
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
        });
    process::terminate(&mut child);
    drop(rx);
    let _ = reader.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_auth_commands_and_error_details_match_the_provider_flow() {
        assert_eq!(
            arguments(Kind::Claude, AuthAction::SignIn),
            Some(["auth", "login"].as_slice())
        );
        assert_eq!(
            arguments(Kind::Codex, AuthAction::SignOut),
            Some(["logout"].as_slice())
        );
        assert_eq!(
            arguments(Kind::Grok, AuthAction::SignIn),
            Some(["login", "--oauth"].as_slice())
        );
        assert!(arguments(Kind::Pi, AuthAction::SignIn).is_none());
        assert_eq!(
            safe_detail("first line\n Complete at https://example.test/?code=private\n"),
            "Complete at sign-in link"
        );
        assert_eq!(safe_detail(&"a".repeat(300)).len(), 240);
    }

    fn scan(chunks: &[&[u8]]) -> Option<AuthProgress> {
        let mut scanner = PromptScanner::default();
        chunks.iter().find_map(|chunk| scanner.feed(chunk))
    }
    fn prompt(url: Option<&str>) -> Option<AuthProgress> {
        Some(AuthProgress::CodePrompt {
            url: url.map(str::to_owned),
        })
    }
    #[test]
    fn only_claude_is_known_to_read_a_pasted_code() {
        assert!(accepts_pasted_code(Kind::Claude));
        for kind in [Kind::Codex, Kind::Cursor, Kind::Grok, Kind::Fx, Kind::Devin] {
            assert!(!accepts_pasted_code(kind));
        }
    }
    #[test]
    fn prompt_scanner_finds_the_url_and_both_prompt_variants() {
        let plain = b"Opening browser to sign in...\nIf the browser didn't open, visit: https://example.test/a?b=1&c=2\nPaste code here if prompted > ";
        assert_eq!(
            scan(&[plain]),
            prompt(Some("https://example.test/a?b=1&c=2"))
        );
        let alternate =
            b"Callback unavailable.\nSign in at: https://example.test/x\nPaste code here > ";
        assert_eq!(scan(&[alternate]), prompt(Some("https://example.test/x")));
        assert_eq!(scan(&[b"Paste code here if prompted > "]), prompt(None));
        assert_eq!(scan(&[b"Opening browser to sign in...\n"]), None);
    }
    #[test]
    fn prompt_scanner_reads_hyperlink_escapes_and_split_chunks() {
        let bel = b"visit: \x1b]8;;https://example.test/h\x07https://example.test/h\x1b]8;;\x07\nPaste code here if prompted > ";
        assert_eq!(scan(&[bel]), prompt(Some("https://example.test/h")));
        let st = b"visit: \x1b[1m\x1b]8;;https://example.test/s\x1b\\open this\x1b]8;;\x1b\\\x1b[0m\nPaste code here > ";
        assert_eq!(scan(&[st]), prompt(Some("https://example.test/s")));
        let whole = b"visit: https://example.test/split\nPaste code here if prompted > ";
        for cut in 1..whole.len() {
            assert_eq!(
                scan(&[&whole[..cut], &whole[cut..]]),
                prompt(Some("https://example.test/split")),
                "cut at {cut}"
            );
        }
        // Reported once, then ignored.
        let mut scanner = PromptScanner::default();
        assert!(scanner.feed(whole).is_some());
        assert!(scanner.feed(whole).is_none());
    }
    #[test]
    fn non_https_links_are_never_offered() {
        assert_eq!(
            scan(&[b"visit: http://example.test/plain\nPaste code here > "]),
            prompt(None)
        );
    }
    #[test]
    fn reject_scanner_counts_complete_invalid_code_lines() {
        let mut scanner = RejectScanner::default();
        assert_eq!(scanner.feed(b"Invalid code. Please make"), 0);
        assert_eq!(scanner.feed(b" sure the full code was copied.\nother\n"), 1);
    }
    #[test]
    fn code_validation_rejects_unusable_input_without_echoing_it() {
        assert_eq!(
            validate_code("  fake-code#fake-state \n").unwrap(),
            "fake-code#fake-state"
        );
        for bad in [
            "",
            "   ",
            "no-separator",
            "#fake-state",
            "fake-code#",
            "a#b#c",
            "fake\ncode#state",
            "fake\u{7}code#state",
            &"x#y".repeat(2000),
        ] {
            let error = validate_code(bad).unwrap_err();
            assert!(!error.is_empty());
            assert!(bad.len() < 4 || !error.contains(bad), "{error}");
        }
    }
    #[test]
    fn submitted_codes_are_redacted_from_error_details() {
        let secrets = vec![
            "fake-code".to_owned(),
            "fake-state".to_owned(),
            "fake-code#fake-state".to_owned(),
        ];
        let detail = redact(
            "Login failed: fake-code#fake-state at https://example.test/?c=fake-code",
            &secrets,
        );
        let message = safe_detail(&detail);
        assert!(
            !message.contains("fake-code") && !message.contains("fake-state"),
            "{message}"
        );
        assert!(message.starts_with("Login failed:"));
    }
    #[test]
    fn the_sign_in_url_comes_from_the_visit_line() {
        let text = b"Docs: https://example.test/docs\nIf the browser didn't open, visit: https://example.test/authorize\nPaste code here if prompted > ";
        assert_eq!(
            scan(&[text]),
            prompt(Some("https://example.test/authorize"))
        );
        let text = b"Docs: https://example.test/docs\nOther: https://example.test/last\nPaste code here > ";
        assert_eq!(scan(&[text]), prompt(Some("https://example.test/last")));
    }
    #[test]
    fn short_halves_and_the_whole_code_are_all_redacted() {
        let secrets = vec!["ab".to_owned(), "cd".to_owned(), "ab#cd".to_owned()];
        assert_eq!(
            redact("bad ab#cd then ab and cd", &secrets),
            "bad [code] then [code] and [code]"
        );
    }
    const MANUAL: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=fake-client&response_type=code&redirect_uri=https%3A%2F%2Fplatform.example.test%2Foauth%2Fcode%2Fcallback&scope=fake&code_challenge=fake-challenge&code_challenge_method=S256&state=fake-state";
    const AUTOMATIC: &str = "https://claude.com/cai/oauth/authorize?client_id=fake-client&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A54321%2Fcallback&scope=fake&code_challenge=fake-challenge&code_challenge_method=S256&state=fake-state";
    #[test]
    fn the_manual_sign_in_url_is_accepted_and_nothing_else() {
        assert_eq!(manual_sign_in_url(MANUAL), Some(MANUAL.to_owned()));
        let with_redirect = |redirect: &str| {
            format!("https://claude.com/cai/oauth/authorize?code=true&redirect_uri={redirect}&state=fake-state")
        };
        for bad in [
            AUTOMATIC.to_owned(),
            MANUAL.replacen("https://", "http://", 1),
            // No `code=true`: not the hosted code page.
            MANUAL.replace("code=true&", ""),
            // Two redirects are ambiguous, whichever comes first.
            format!("{MANUAL}&redirect_uri=http%3A%2F%2Flocalhost%3A1%2Fcallback"),
            MANUAL.replace(
                "redirect_uri=https%3A",
                "redirect_uri=http%3A%2F%2Flocalhost%3A1%2Fcallback&redirect_uri=https%3A",
            ),
            "https://claude.com/cai/oauth/authorize?state=fake-state".to_owned(),
            "not a url".to_owned(),
            "ftp://claude.com/a?redirect_uri=https%3A%2F%2Fa.test%2Fb".to_owned(),
            // Encoded and plain loopback redirects, over http and https.
            with_redirect("http%3A%2F%2Flocalhost%3A1%2Fcallback"),
            with_redirect("http://localhost:1/callback"),
            with_redirect("https%3A%2F%2Flocalhost%3A1%2Fcallback"),
            with_redirect("https%3A%2F%2FLOCALHOST.%2Fcallback"),
            with_redirect("https%3A%2F%2Fapp.localhost%2Fcallback"),
            with_redirect("https%3A%2F%2F127.0.0.1%3A1%2Fcallback"),
            with_redirect("https://127.0.0.1:1/callback"),
            with_redirect("https%3A%2F%2F127.5.6.7%2Fcallback"),
            with_redirect("https%3A%2F%2F%5B%3A%3A1%5D%3A1%2Fcallback"),
            with_redirect("https://[::1]:1/callback"),
            with_redirect("https://[::ffff:127.0.0.1]/callback"),
            with_redirect("https%3A%2F%2F0.0.0.0%2Fcallback"),
            with_redirect("not-a-url"),
        ] {
            assert_eq!(manual_sign_in_url(&bad), None, "{bad}");
        }
    }
    #[cfg(unix)]
    mod process {
        use super::*;
        use std::sync::Mutex;
        const FAKE: &str = r#"printf 'Opening browser to sign in...\nIf the browser did not open, visit: https://example.test/authorize\nPaste code here if prompted > '
while IFS= read -r line; do
  case "$line" in
    fake-code#fake-state) echo 'Login successful.'; exit 0;;
    *) echo "Login failed: $line" >&2; exit 1;;
  esac
done"#;
        type Callback = Arc<dyn Fn(AuthProgress) + Send + Sync>;
        fn events() -> (Callback, mpsc::Receiver<AuthProgress>) {
            let (tx, rx) = mpsc::channel();
            let tx = Mutex::new(tx);
            (
                Arc::new(move |event| {
                    let _ = tx.lock().unwrap().send(event);
                }),
                rx,
            )
        }
        type Started = (
            thread::JoinHandle<Result<(), String>>,
            mpsc::Sender<String>,
            mpsc::Receiver<AuthProgress>,
        );
        fn start(script: &str, timeout: Duration, cancel: Arc<AtomicBool>) -> Started {
            let (progress, events) = events();
            let (codes_tx, codes) = mpsc::channel();
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            let handle = thread::spawn(move || {
                run(
                    command,
                    Kind::Claude,
                    &Profile::default(),
                    timeout,
                    cancel,
                    Some(AuthIo { codes, progress }),
                )
            });
            (handle, codes_tx, events)
        }
        fn next(events: &mpsc::Receiver<AuthProgress>) -> AuthProgress {
            events.recv_timeout(Duration::from_secs(20)).unwrap()
        }
        #[test]
        fn a_pasted_code_reaches_the_cli_and_finishes_the_sign_in() {
            let (handle, codes, events) = start(FAKE, Duration::from_secs(30), Arc::default());
            assert_eq!(
                next(&events),
                AuthProgress::CodePrompt {
                    url: Some("https://example.test/authorize".into())
                }
            );
            codes.send("fake-code#fake-state".into()).unwrap();
            assert_eq!(handle.join().unwrap(), Ok(()));
        }
        #[test]
        fn a_wrong_code_fails_clearly_without_leaking_it() {
            let (handle, codes, events) = start(FAKE, Duration::from_secs(30), Arc::default());
            next(&events);
            codes.send("wrong-code#wrong-state".into()).unwrap();
            let error = handle.join().unwrap().unwrap_err();
            assert!(error.starts_with(NOT_ACCEPTED), "{error}");
            assert!(
                !error.contains("wrong-code") && !error.contains("wrong-state"),
                "{error}"
            );
        }
        #[test]
        fn a_malformed_code_is_refused_before_the_cli_and_the_prompt_stays() {
            let (handle, codes, events) = start(FAKE, Duration::from_secs(30), Arc::default());
            next(&events);
            codes.send("not-a-code".into()).unwrap();
            assert!(matches!(next(&events), AuthProgress::CodeRejected(_)));
            codes.send("fake-code#fake-state".into()).unwrap();
            assert_eq!(handle.join().unwrap(), Ok(()));
        }
        #[test]
        fn the_cli_reporting_an_invalid_code_is_forwarded() {
            let script =
                "echo 'Invalid code. Please make sure the full code was copied.' >&2; sleep 1; exit 1";
            let (handle, _codes, events) = start(script, Duration::from_secs(30), Arc::default());
            assert_eq!(next(&events), AuthProgress::CodeRejected(REJECTED.into()));
            assert!(handle.join().unwrap().is_err());
        }
        #[test]
        fn a_provider_without_code_support_keeps_a_closed_stdin() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("seen");
            let mut command = Command::new("/bin/sh");
            command.args([
                "-c",
                &format!(
                    "if read line; then echo data > '{0}'; else echo eof > '{0}'; fi",
                    file.display()
                ),
            ]);
            run(
                command,
                Kind::Codex,
                &Profile::default(),
                Duration::from_secs(30),
                Arc::default(),
                None,
            )
            .unwrap();
            assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "eof");
        }
        /// Prints both URLs like the real CLI (manual on the "visit:" line),
        /// then does what the CLI does with the browser: runs
        /// `$BROWSER <automatic url>` if it is set, and records that.
        fn two_url_cli(dir: &std::path::Path) -> String {
            format!(
                r#"printf 'Opening browser to sign in...\nIf the browser did not open, visit: %s\nPaste code here if prompted > ' '{MANUAL}'
echo "${{BROWSER-unset}}" > '{0}/browser'
if [ -n "$BROWSER" ]; then
  "$BROWSER" '{AUTOMATIC}' && echo ran > '{0}/opened-by-cli'
else
  echo would-open-default > '{0}/opened-by-cli'
fi
while IFS= read -r line; do
  case "$line" in
    fake-code#fake-state) exit 0;;
    *) exit 1;;
  esac
done"#,
                dir.display()
            )
        }
        fn run_two_url_cli(mode: SignInMode) -> (String, String, AuthProgress) {
            let dir = tempfile::tempdir().unwrap();
            let script = two_url_cli(dir.path());
            let (progress, events) = events();
            let (codes_tx, codes) = mpsc::channel();
            let mut command = Command::new("/bin/sh");
            command.args(["-c", &script]);
            // Parent BROWSER must not leak into the comparison.
            command.env_remove("BROWSER");
            apply_mode(&mut command, Kind::Claude, mode);
            let handle = thread::spawn(move || {
                run(
                    command,
                    Kind::Claude,
                    &Profile::default(),
                    Duration::from_secs(30),
                    Arc::default(),
                    Some(AuthIo { codes, progress }),
                )
            });
            let event = next(&events);
            codes_tx.send("fake-code#fake-state".into()).unwrap();
            assert_eq!(handle.join().unwrap(), Ok(()));
            let read = |name: &str| {
                std::fs::read_to_string(dir.path().join(name))
                    .unwrap()
                    .trim()
                    .to_owned()
            };
            (read("browser"), read("opened-by-cli"), event)
        }
        #[test]
        fn code_mode_stops_the_cli_opening_a_page_and_offers_the_manual_url() {
            let (browser, opened, event) = run_two_url_cli(SignInMode::Code);
            assert_eq!(browser, "true");
            // `true` ran: it is a no-op, and the CLI never got a page of its own.
            assert_eq!(opened, "ran");
            let AuthProgress::CodePrompt { url } = event else {
                panic!("expected the code prompt");
            };
            assert_eq!(url.as_deref(), Some(MANUAL));
            assert_eq!(
                manual_sign_in_url(url.as_deref().unwrap()).as_deref(),
                url.as_deref()
            );
        }
        #[test]
        fn browser_mode_leaves_the_cli_browser_alone() {
            let (browser, opened, event) = run_two_url_cli(SignInMode::Browser);
            assert_eq!(browser, "unset", "no BROWSER is injected");
            assert_eq!(opened, "would-open-default");
            assert_eq!(
                event,
                AuthProgress::CodePrompt {
                    url: Some(MANUAL.into())
                }
            );
        }
        #[test]
        fn code_mode_only_changes_the_environment_for_claude() {
            let env = |kind, mode| {
                let mut command = Command::new("cli");
                apply_mode(&mut command, kind, mode);
                command.get_envs().count()
            };
            assert_eq!(env(Kind::Claude, SignInMode::Browser), 0);
            assert_eq!(env(Kind::Codex, SignInMode::Code), 0);
            assert_eq!(env(Kind::Claude, SignInMode::Code), 1);
        }
        #[test]
        fn io_is_dropped_for_a_kind_or_action_without_code_support() {
            let io = || {
                let (progress, _events) = events();
                let (_tx, codes) = mpsc::channel();
                Some(AuthIo { codes, progress })
            };
            assert!(code_io(Kind::Codex, AuthAction::SignIn, io()).is_none());
            assert!(code_io(Kind::Claude, AuthAction::SignOut, io()).is_none());
            assert!(code_io(Kind::Claude, AuthAction::SignIn, io()).is_some());
        }
        fn alive(pid: i32) -> bool {
            unsafe { libc::kill(pid, 0) == 0 }
        }
        /// Runs a CLI that records its pid then waits on stdin; ends the
        /// attempt by cancel (long timeout) or by timeout (short one).
        fn waiting_cli(timeout: Duration) -> (i32, Result<(), String>) {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("pid");
            let script = format!("echo $$ > '{}'; read line; sleep 30", file.display());
            let cancel = Arc::new(AtomicBool::new(false));
            let (handle, _codes, _events) = start(&script, timeout, cancel.clone());
            let started = Instant::now();
            let pid = loop {
                if let Some(pid) = std::fs::read_to_string(&file)
                    .ok()
                    .and_then(|text| text.trim().parse().ok())
                {
                    break pid;
                }
                assert!(started.elapsed() < Duration::from_secs(20));
                thread::sleep(Duration::from_millis(20));
            };
            if timeout > Duration::from_secs(5) {
                cancel.store(true, Ordering::Relaxed);
            }
            (pid, handle.join().unwrap())
        }
        #[test]
        fn a_sign_in_cancelled_beforehand_never_starts_the_cli() {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join("ran");
            let script = format!("touch '{}'; sleep 5", marker.display());
            let cancel = Arc::new(AtomicBool::new(true));
            let (handle, _codes, _events) = start(&script, Duration::from_secs(30), cancel);
            assert_eq!(handle.join().unwrap(), Err("Sign-in cancelled".into()));
            thread::sleep(Duration::from_millis(300));
            assert!(!marker.exists(), "the CLI must not be executed");
        }
        #[test]
        fn cancel_kills_a_cli_waiting_on_stdin() {
            let (pid, result) = waiting_cli(Duration::from_secs(30));
            assert_eq!(result, Err("Sign-in cancelled".into()));
            assert!(
                !alive(pid),
                "the sign-in process must not outlive the attempt"
            );
        }
        #[test]
        fn timeout_kills_a_cli_waiting_on_stdin() {
            let (pid, result) = waiting_cli(Duration::from_secs(2));
            assert!(result.unwrap_err().contains("timed out"));
            assert!(!alive(pid));
        }
    }
}
