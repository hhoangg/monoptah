//! Run a workflow command node: one shell command in a working directory,
//! with a timeout that kills the whole process tree and a bounded output tail.
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Characters of stdout and stderr each that are kept (the end of the output).
const OUTPUT_TAIL_CHARS: usize = 8000;
/// Bytes buffered per stream; a UTF-8 char is at most 4 bytes, so this always
/// holds the last `OUTPUT_TAIL_CHARS` chars plus a possibly cut leading char.
const OUTPUT_TAIL_BYTES: usize = OUTPUT_TAIL_CHARS * 4;
/// How long the pipe readers may outlive the shell before the group is killed.
const READER_GRACE: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[tauri::command]
pub async fn workflow_run_command(
    command: String,
    cwd: String,
    timeout_ms: u64,
) -> Result<CommandResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_command(&command, &cwd, Duration::from_millis(timeout_ms))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Pick the shell. Command templates are quoted for POSIX shells, so the
/// user's login shell is only used when it is one; anything else (fish, nu,
/// csh...) would read the quotes differently and falls back to `/bin/sh`.
#[cfg(unix)]
fn shell_for(shell: Option<&str>) -> (String, &'static str) {
    let name = shell
        .and_then(|s| Path::new(s).file_name())
        .and_then(|n| n.to_str());
    match (shell, name) {
        (Some(path), Some("bash" | "zsh" | "sh" | "dash" | "ksh")) => (path.to_string(), "-lc"),
        _ => ("/bin/sh".to_string(), "-c"),
    }
}

fn run_command(command: &str, cwd: &str, timeout: Duration) -> Result<CommandResult, String> {
    let dir = Path::new(cwd);
    if !dir.is_absolute() {
        return Err(format!("Working directory must be absolute: {cwd}"));
    }
    if !dir.is_dir() {
        return Err(format!("Working directory does not exist: {cwd}"));
    }
    #[cfg(unix)]
    let (program, flag) = shell_for(std::env::var("SHELL").ok().as_deref());
    #[cfg(windows)]
    let (program, flag) = ("cmd".to_string(), "/C");
    execute(&program, flag, command, dir, timeout)
}

fn execute(
    program: &str,
    flag: &str,
    command: &str,
    dir: &Path,
    timeout: Duration,
) -> Result<CommandResult, String> {
    let mut cmd = Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg(flag).arg(command);
        // Own process group: the group id stays valid for a timeout kill even
        // after the shell itself has exited.
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // `raw_arg` keeps the command as written; `arg` would re-quote it for
        // the MSVC argv rules, which cmd.exe does not follow.
        cmd.arg(flag).raw_arg(command);
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start {program}: {e}"))?;

    let stdout = child.stdout.take().map(spawn_tail_reader);
    let stderr = child.stderr.take().map(spawn_tail_reader);

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                kill_tree(&mut child);
                return Err(format!("Could not wait for the command: {e}"));
            }
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            kill_tree(&mut child);
            break child.wait().ok();
        }
        thread::sleep(POLL_INTERVAL);
    };

    // A descendant that outlives the shell can keep a pipe open forever; give
    // the readers a moment, then end the group so the join cannot hang.
    let out = collect(stdout, &mut child);
    let err = collect(stderr, &mut child);
    Ok(CommandResult {
        exit_code: if timed_out {
            None
        } else {
            status.and_then(|s| s.code())
        },
        stdout: out,
        stderr: err,
        timed_out,
    })
}

fn spawn_tail_reader<R: Read + Send + 'static>(mut stream: R) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.extend_from_slice(&chunk[..n]);
                    if tail.len() > OUTPUT_TAIL_BYTES * 2 {
                        tail.drain(..tail.len() - OUTPUT_TAIL_BYTES);
                    }
                }
            }
        }
        let _ = tx.send(tail);
    });
    rx
}

fn collect(reader: Option<mpsc::Receiver<Vec<u8>>>, child: &mut Child) -> String {
    let Some(rx) = reader else {
        return String::new();
    };
    let bytes = match rx.recv_timeout(READER_GRACE) {
        Ok(bytes) => bytes,
        Err(_) => {
            kill_tree(child);
            rx.recv().unwrap_or_default()
        }
    };
    tail_chars(&bytes, OUTPUT_TAIL_CHARS)
}

/// Last `max` chars of `bytes`, cut on a char boundary. The buffer start may
/// split a UTF-8 char; lossy decoding turns that into U+FFFD, which is
/// dropped by the tail whenever the output is longer than `max`.
fn tail_chars(bytes: &[u8], max: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let count = text.chars().count();
    if count <= max {
        return text.into_owned();
    }
    text.chars().skip(count - max).collect()
}

#[cfg(unix)]
fn kill_tree(child: &mut Child) {
    // process_group(0) made the pid the group id; the negative pid reaches the
    // shell and every descendant, such as a `sleep` it started.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(windows)]
fn kill_tree(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(command: &str, timeout_ms: u64) -> CommandResult {
        execute(
            "/bin/sh",
            "-c",
            command,
            Path::new("/tmp"),
            Duration::from_millis(timeout_ms),
        )
        .expect("command runs")
    }

    #[test]
    fn captures_stdout_and_exit_code() {
        let result = sh("echo hi; exit 3", 5000);
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(result.stdout, "hi\n");
        assert_eq!(result.stderr, "");
        assert!(!result.timed_out);
    }

    #[test]
    fn captures_stderr() {
        let result = sh("echo oops >&2", 5000);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stderr, "oops\n");
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn times_out() {
        let started = Instant::now();
        let result = sh("sleep 5", 200);
        assert!(result.timed_out);
        assert_eq!(result.exit_code, None);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn timeout_kills_grandchildren() {
        let result = sh("sleep 30 & echo $!; wait", 200);
        assert!(result.timed_out);
        let pid: i32 = result.stdout.trim().parse().expect("grandchild pid");
        // Signal 0 only probes: it fails once the process is gone or reaped.
        let mut gone = false;
        for _ in 0..50 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                gone = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(gone, "grandchild {pid} survived the timeout");
    }

    #[test]
    fn leftover_background_process_does_not_hang() {
        let started = Instant::now();
        let result = sh("sleep 30 & echo started", 5000);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "started\n");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn tails_long_output_to_8000_chars() {
        let result = sh(
            "i=0; while [ $i -lt 20000 ]; do printf 'x'; i=$((i+1)); done; printf 'END'",
            20000,
        );
        assert_eq!(result.stdout.chars().count(), 8000);
        assert!(result.stdout.ends_with("xxxEND"));
    }

    #[test]
    fn tail_cuts_on_a_char_boundary() {
        let text = "é".repeat(OUTPUT_TAIL_BYTES);
        let cut = tail_chars(text.as_bytes(), 8000);
        assert_eq!(cut.chars().count(), 8000);
        assert!(cut.chars().all(|c| c == 'é'));
        // A buffer that starts mid-char still yields only whole chars.
        let cut = tail_chars(&text.as_bytes()[1..], 8000);
        assert_eq!(cut.chars().count(), 8000);
        assert!(cut.chars().all(|c| c == 'é'));
    }

    #[test]
    fn large_output_on_both_pipes_does_not_deadlock() {
        let result = sh(
            "yes out | head -c 2000000; yes err | head -c 2000000 >&2",
            20000,
        );
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.chars().count(), 8000);
        assert_eq!(result.stderr.chars().count(), 8000);
    }

    #[test]
    fn rejects_relative_cwd() {
        let err = run_command("echo hi", "relative/dir", Duration::from_secs(5)).unwrap_err();
        assert!(err.contains("absolute"), "{err}");
    }

    #[test]
    fn rejects_missing_cwd() {
        let err =
            run_command("echo hi", "/no/such/dir/anywhere", Duration::from_secs(5)).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn runs_in_the_given_cwd() {
        let dir = std::env::temp_dir().canonicalize().expect("temp dir");
        let result = run_command("pwd -P", dir.to_str().unwrap(), Duration::from_secs(10))
            .expect("command runs");
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.trim_end().ends_with(dir.to_str().unwrap()));
    }

    #[test]
    fn shell_choice_follows_posix_compatibility() {
        let sh = |p: &str, f: &'static str| (p.to_string(), f);
        assert_eq!(shell_for(Some("/bin/zsh")), sh("/bin/zsh", "-lc"));
        assert_eq!(shell_for(Some("/usr/bin/bash")), sh("/usr/bin/bash", "-lc"));
        assert_eq!(shell_for(Some("/bin/dash")), sh("/bin/dash", "-lc"));
        assert_eq!(shell_for(Some("/bin/ksh")), sh("/bin/ksh", "-lc"));
        assert_eq!(shell_for(Some("/bin/sh")), sh("/bin/sh", "-lc"));
        assert_eq!(shell_for(Some("/usr/local/bin/fish")), sh("/bin/sh", "-c"));
        assert_eq!(shell_for(Some("/usr/bin/nu")), sh("/bin/sh", "-c"));
        assert_eq!(shell_for(Some("")), sh("/bin/sh", "-c"));
        assert_eq!(shell_for(None), sh("/bin/sh", "-c"));
    }
}
