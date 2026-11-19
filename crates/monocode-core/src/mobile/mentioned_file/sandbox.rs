//! Runs the HTML transform where it cannot take the app down.
//!
//! The transform (`css-inline` and `ammonia` on `html5ever`) cannot be bounded by estimating:
//! a 10 KB document of nested `<div>` overflows the 512 KB stack of the app's background
//! threads and aborts the whole process, and cloned formatting elements, selector lists and
//! sibling or descendant selectors each found a way past every estimate. So the work runs in a
//! child process of the same executable, started with [`CHILD_ARG`]:
//!
//! * the parent does everything that touches the file system (reading the file, the folder and
//!   deny-list checks, loading the linked stylesheets) and sends bytes: the HTML and the
//!   stylesheets it already loaded and checked. The child opens no file;
//! * the child ([`maybe_run_child`], first thing in `main`) limits its memory (`RLIMIT_AS`
//!   where the system enforces it), runs the transform on a thread with a 64 MiB stack and
//!   writes the document and the asset list to stdout;
//! * the parent reads at most [`MAX_OUTPUT_BYTES`] of the reply, waits at most [`TIMEOUT`], and
//!   kills the child past either. A child that crashes, aborts or exits non-zero is a refusal
//!   (`too_large`, "This page is too complex to preview"), never a panic in the parent.
//!
//! Tests and tools that are not the app run [`Transformer::InProcess`] (the same transform on a
//! thread with a 64 MiB stack). Both modes are fed only documents that passed
//! [`super::prepass`], which caps the element count and the depth.
use std::{
    collections::HashMap,
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, OnceLock},
    thread,
    time::{Duration, Instant},
};

use super::{html, MENTIONED_FILE_MAX_BYTES};
use crate::mobile::policy::{Code, Failure};

/// The first argument that makes the executable a transform child.
pub const CHILD_ARG: &str = "--monoptah-html-transform";
/// Stack of the thread that runs the transform.
const STACK_BYTES: usize = 64 * 1024 * 1024;
/// Memory a child may map where the system enforces it.
#[cfg(unix)]
const MEMORY_BYTES: u64 = 1 << 30;
/// How long a child may run.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// The most a child may write: the document cap plus the asset list.
pub const MAX_OUTPUT_BYTES: usize = MENTIONED_FILE_MAX_BYTES as usize + 1024 * 1024;
/// The most a child reads (a 4 MiB document and up to 8 MiB of stylesheets, with framing).
const MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SHEETS: usize = 1024;
const REQUEST_MAGIC: &[u8; 4] = b"MPQ1";
const REPLY_MAGIC: &[u8; 4] = b"MPR1";

/// What the transform needs: the HTML and the linked stylesheets the parent already loaded and
/// checked, by the `href` the document names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Input {
    pub html: String,
    pub sheets: Vec<(String, String)>,
}

/// The refusal for a document the transform could not finish within its limits.
pub fn too_complex() -> Failure {
    Failure::coded(
        Code::TooLarge.as_str(),
        "This page is too complex to preview",
    )
}

/// How the transform runs.
#[derive(Debug, Clone)]
pub enum Transformer {
    /// On a thread of this process (tests, tools): no time or memory limit.
    InProcess,
    /// In a child process: the executable to run, with [`CHILD_ARG`].
    Subprocess(PathBuf),
}

static TRANSFORMER: OnceLock<Transformer> = OnceLock::new();

/// Makes the app run the transform in a child process (the current executable). Call it once at
/// startup, after [`maybe_run_child`]. Without it the transform runs in process.
pub fn enable_subprocess() {
    // Fail closed: when the executable is unknown the child cannot start and every HTML
    // document is refused, instead of running in process.
    let exe = std::env::current_exe().unwrap_or_default();
    let _ = TRANSFORMER.set(Transformer::Subprocess(exe));
}

/// The transformer in use.
pub fn transformer() -> &'static Transformer {
    TRANSFORMER.get_or_init(|| Transformer::InProcess)
}

impl Transformer {
    pub fn run(&self, input: Input) -> Result<html::Document, Failure> {
        match self {
            Transformer::InProcess => in_process(input),
            Transformer::Subprocess(exe) => transform_isolated(exe, &input),
        }
    }
}

struct MapSheets(HashMap<String, String>);

impl html::Sheets for MapSheets {
    fn load(&self, href: &str) -> Option<String> {
        self.0.get(href).cloned()
    }
}

fn transform_input(input: Input) -> html::Document {
    let sheets = Arc::new(MapSheets(input.sheets.into_iter().collect()));
    html::transform(&input.html, sheets)
}

fn in_process(input: Input) -> Result<html::Document, Failure> {
    thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || transform_input(input))
        .map_err(|_| too_complex())?
        .join()
        .map_err(|_| too_complex())
}

// ---- framing ----

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], magic: &[u8; 4]) -> Option<Self> {
        data.strip_prefix(magic.as_slice())
            .map(|data| Self { data, at: 0 })
    }

    fn number(&mut self) -> Option<usize> {
        let bytes = self.data.get(self.at..self.at + 4)?;
        self.at += 4;
        Some(u32::from_le_bytes(bytes.try_into().ok()?) as usize)
    }

    fn frame(&mut self) -> Option<&'a [u8]> {
        let len = self.number()?;
        let bytes = self.data.get(self.at..self.at.checked_add(len)?)?;
        self.at += len;
        Some(bytes)
    }

    fn text(&mut self) -> Option<String> {
        String::from_utf8(self.frame()?.to_vec()).ok()
    }

    fn done(&self) -> bool {
        self.at == self.data.len()
    }
}

pub fn encode_request(input: &Input) -> Vec<u8> {
    let mut out = REQUEST_MAGIC.to_vec();
    put(&mut out, input.html.as_bytes());
    out.extend_from_slice(&(input.sheets.len() as u32).to_le_bytes());
    for (href, text) in &input.sheets {
        put(&mut out, href.as_bytes());
        put(&mut out, text.as_bytes());
    }
    out
}

pub fn decode_request(data: &[u8]) -> Option<Input> {
    let mut cursor = Cursor::new(data, REQUEST_MAGIC)?;
    let html = cursor.text()?;
    let count = cursor.number()?;
    if count > MAX_SHEETS {
        return None;
    }
    let mut sheets = Vec::new();
    for _ in 0..count {
        sheets.push((cursor.text()?, cursor.text()?));
    }
    cursor.done().then_some(Input { html, sheets })
}

pub fn encode_reply(document: &html::Document) -> Vec<u8> {
    let mut out = REPLY_MAGIC.to_vec();
    put(&mut out, document.html.as_bytes());
    out.extend_from_slice(&(document.assets.len() as u32).to_le_bytes());
    for asset in &document.assets {
        put(&mut out, asset.as_bytes());
    }
    out
}

pub fn decode_reply(data: &[u8]) -> Option<html::Document> {
    let mut cursor = Cursor::new(data, REPLY_MAGIC)?;
    let html = cursor.text()?;
    let count = cursor.number()?;
    if count > html::MAX_ASSETS {
        return None;
    }
    let mut assets = Vec::new();
    for _ in 0..count {
        assets.push(cursor.text()?);
    }
    cursor.done().then_some(html::Document { html, assets })
}

// ---- the child ----

/// When this process was started as a transform child, serves one request and exits; otherwise
/// returns at once. Call it first in `main`, before any other initialization.
pub fn maybe_run_child() {
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new(CHILD_ARG)) {
        return;
    }
    std::process::exit(run_child());
}

fn run_child() -> i32 {
    limit_memory();
    let mut request = Vec::new();
    if std::io::stdin()
        .lock()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut request)
        .is_err()
        || request.len() as u64 > MAX_REQUEST_BYTES
    {
        return 2;
    }
    let Some(input) = decode_request(&request) else {
        return 2;
    };
    drop(request);
    let Ok(document) = in_process(input) else {
        return 3;
    };
    let reply = encode_reply(&document);
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(&reply)
        .and_then(|_| stdout.flush())
        .is_err()
    {
        return 4;
    }
    0
}

/// Caps the address space of the child where the system enforces it (Linux). macOS does not
/// enforce `RLIMIT_AS` or `RLIMIT_DATA`, so there the element cap, the output cap and the
/// timeout are the bound; errors are ignored.
fn limit_memory() {
    #[cfg(unix)]
    {
        let limit = libc::rlimit {
            rlim_cur: MEMORY_BYTES as libc::rlim_t,
            rlim_max: MEMORY_BYTES as libc::rlim_t,
        };
        // SAFETY: `setrlimit` only reads the struct it is given.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        unsafe {
            libc::setrlimit(libc::RLIMIT_AS, &limit);
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        unsafe {
            libc::setrlimit(libc::RLIMIT_DATA, &limit);
        }
    }
}

// ---- the parent ----

/// Limits of one child run.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    pub max_output: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        timeout: TIMEOUT,
        max_output: MAX_OUTPUT_BYTES,
    };
}

/// Transforms `input` in a child process running `exe` (the app's own executable).
pub fn transform_isolated(exe: &Path, input: &Input) -> Result<html::Document, Failure> {
    let mut command = Command::new(exe);
    command.arg(CHILD_ARG);
    run_command(command, encode_request(input), &Limits::DEFAULT)
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs `command` as a transform child: `request` on its stdin, its stdout decoded as a reply,
/// the limits enforced, the child always killed and reaped.
pub fn run_command(
    mut command: Command,
    request: Vec<u8>,
    limits: &Limits,
) -> Result<html::Document, Failure> {
    command
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let started = Instant::now();
    let mut child = KillOnDrop(command.spawn().map_err(|_| too_complex())?);
    let (mut stdin, stdout) = match (child.0.stdin.take(), child.0.stdout.take()) {
        (Some(stdin), Some(stdout)) => (stdin, stdout),
        _ => return Err(too_complex()),
    };
    // The writer and the reader end by themselves when the child is killed (broken pipe, EOF).
    thread::Builder::new()
        .spawn(move || {
            let _ = stdin.write_all(&request);
        })
        .map_err(|_| too_complex())?;
    let (sender, receiver) = mpsc::channel();
    let cap = limits.max_output;
    thread::Builder::new()
        .spawn(move || {
            let mut out = Vec::new();
            let read = stdout.take(cap as u64 + 1).read_to_end(&mut out);
            let _ = sender.send(read.map(|_| out));
        })
        .map_err(|_| too_complex())?;
    let remaining = limits.timeout.saturating_sub(started.elapsed());
    let reply = match receiver.recv_timeout(remaining) {
        Ok(Ok(reply)) if reply.len() <= cap => reply,
        _ => return Err(too_complex()),
    };
    // The output is complete; the child must also end well, within the same time.
    loop {
        match child.0.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return Err(too_complex()),
            Ok(None) if started.elapsed() >= limits.timeout => return Err(too_complex()),
            Ok(None) => thread::sleep(Duration::from_millis(2)),
        }
    }
    decode_reply(&reply).ok_or_else(too_complex)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Input {
        Input {
            html: "<link rel=stylesheet href=a.css><p class=x>héllo</p>".into(),
            sheets: vec![("a.css".into(), ".x{color:red}".into())],
        }
    }

    #[test]
    fn the_request_and_the_reply_round_trip() {
        let input = input();
        assert_eq!(decode_request(&encode_request(&input)), Some(input));
        let document = html::Document {
            html: "<p>é</p>".into(),
            assets: vec!["a/b.png".into(), "c.png".into()],
        };
        assert_eq!(decode_reply(&encode_reply(&document)), Some(document));
    }

    #[test]
    fn a_malformed_frame_is_not_decoded() {
        let good = encode_request(&input());
        assert_eq!(decode_request(&good[..good.len() - 1]), None);
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(decode_request(&trailing), None);
        assert_eq!(decode_request(b"nope"), None);
        let mut lying = REPLY_MAGIC.to_vec();
        lying.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode_reply(&lying), None);
        let mut bad_utf8 = REPLY_MAGIC.to_vec();
        put(&mut bad_utf8, &[0xff, 0xfe]);
        bad_utf8.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode_reply(&bad_utf8), None);
    }

    #[test]
    fn the_in_process_transform_uses_only_the_sheets_it_was_given() {
        let document = in_process(input()).unwrap();
        assert!(document.html.contains("color: red"), "{}", document.html);
        let without = in_process(Input {
            sheets: Vec::new(),
            ..input()
        })
        .unwrap();
        assert!(!without.html.contains("color: red"));
    }

    #[test]
    fn a_deeply_nested_page_that_passed_the_pre_pass_does_not_overflow_the_stack() {
        let html = format!("{}x{}", "<div>".repeat(200), "</div>".repeat(200));
        assert!(super::super::prepass::scan(&html).is_ok());
        let document = in_process(Input {
            html,
            sheets: Vec::new(),
        })
        .unwrap();
        assert!(document.html.contains('x'));
    }

    #[cfg(unix)]
    mod child_protocol {
        use super::*;

        fn shell(script: &str) -> Command {
            let mut command = Command::new("sh");
            command.arg("-c").arg(script);
            command
        }

        fn limits(timeout_ms: u64, max_output: usize) -> Limits {
            Limits {
                timeout: Duration::from_millis(timeout_ms),
                max_output,
            }
        }

        fn refused(result: Result<html::Document, Failure>) {
            let failure = result.unwrap_err();
            assert_eq!(failure.code.as_deref(), Some("too_large"));
            assert_eq!(failure.error, "This page is too complex to preview");
        }

        #[test]
        fn a_child_that_does_not_answer_in_time_is_killed_and_refused() {
            let started = Instant::now();
            let result = run_command(shell("exec sleep 20"), Vec::new(), &limits(300, 1 << 20));
            refused(result);
            assert!(started.elapsed() < Duration::from_secs(5));
        }

        #[test]
        fn a_child_that_writes_past_the_cap_is_refused() {
            let started = Instant::now();
            let result = run_command(
                shell("head -c 100000 /dev/zero; exec sleep 20"),
                Vec::new(),
                &limits(10_000, 1000),
            );
            refused(result);
            // Killed at the cap, not at the timeout.
            assert!(started.elapsed() < Duration::from_secs(5));
        }

        #[test]
        fn a_child_that_exits_non_zero_or_crashes_is_refused() {
            refused(run_command(shell("exit 3"), Vec::new(), &Limits::DEFAULT));
            refused(run_command(
                shell("kill -SEGV $$"),
                Vec::new(),
                &Limits::DEFAULT,
            ));
            refused(run_command(
                Command::new("/nonexistent/monoptah-child"),
                Vec::new(),
                &Limits::DEFAULT,
            ));
        }

        #[test]
        fn a_child_that_exits_non_zero_after_a_good_reply_is_still_refused() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("reply.bin");
            let document = html::Document {
                html: "<p>x</p>".into(),
                assets: Vec::new(),
            };
            std::fs::write(&file, encode_reply(&document)).unwrap();
            let script = format!("cat '{}'; exit 1", file.display());
            refused(run_command(shell(&script), Vec::new(), &Limits::DEFAULT));
        }

        #[test]
        fn a_good_reply_is_decoded_and_a_garbled_one_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("reply.bin");
            let document = html::Document {
                html: "<p>x</p>".into(),
                assets: vec!["a.png".into()],
            };
            std::fs::write(&file, encode_reply(&document)).unwrap();
            let script = format!("cat '{}'", file.display());
            // The request is bigger than a pipe buffer and never read: the writer must not
            // hang the parent.
            let request = vec![b'a'; 4 * 1024 * 1024];
            assert_eq!(
                run_command(shell(&script), request, &Limits::DEFAULT).unwrap(),
                document
            );
            refused(run_command(
                shell("echo garbage"),
                Vec::new(),
                &Limits::DEFAULT,
            ));
        }

        #[test]
        fn the_child_environment_is_cleared() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("reply.bin");
            let document = html::Document {
                html: "<p>x</p>".into(),
                assets: Vec::new(),
            };
            std::fs::write(&file, encode_reply(&document)).unwrap();
            // `PATH` is set in every test process; the child must not see it.
            let script = format!(
                "if [ -n \"${{PATH:-}}\" ] && [ \"$PATH\" != \"/usr/bin:/bin\" ] && [ -n \"${{HOME:-}}\" ]; then exit 1; fi; cat '{}'",
                file.display()
            );
            assert_eq!(
                run_command(shell(&script), Vec::new(), &Limits::DEFAULT).unwrap(),
                document
            );
        }
    }
}
