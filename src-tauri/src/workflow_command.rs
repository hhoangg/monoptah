//! Run a workflow command node: one shell command in a working directory,
//! with a timeout that kills the whole process tree and a bounded output tail.
//! A leftover background process that inherited the output pipes is killed
//! after a short grace period; one that redirected its output is left running.
//!
//! Command nodes are not supported on Windows yet: the command templates are
//! quoted for POSIX shells, and `cmd.exe` would expand `%VAR%` and run `a & b`.
use std::time::Duration;

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
        imp::run_command(&command, &cwd, Duration::from_millis(timeout_ms))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(unix)]
mod imp {
    use std::io::{ErrorKind, Read};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::CommandResult;

    /// Characters of stdout and stderr each that are kept (the end of the output).
    const OUTPUT_TAIL_CHARS: usize = 8000;
    /// Bytes buffered per stream; a UTF-8 char is at most 4 bytes, so this always
    /// holds the last `OUTPUT_TAIL_CHARS` chars plus a possibly cut leading char.
    const OUTPUT_TAIL_BYTES: usize = OUTPUT_TAIL_CHARS * 4;
    /// How long the pipe readers may outlive the shell before the group is killed.
    const READER_GRACE: Duration = Duration::from_millis(500);
    /// Extra wait for the readers after the group was killed.
    const POST_KILL_WAIT: Duration = Duration::from_secs(1);
    const POLL_INTERVAL: Duration = Duration::from_millis(10);

    /// Pick the shell. Command templates are quoted for POSIX shells, so the
    /// user's login shell is only used when it is one; anything else (fish, nu,
    /// csh...) would read the quotes differently and falls back to `/bin/sh`.
    fn shell_for(shell: Option<&str>) -> (String, &'static str) {
        let name = shell
            .and_then(|s| Path::new(s).file_name())
            .and_then(|n| n.to_str());
        match (shell, name) {
            (Some(path), Some("bash" | "zsh" | "sh" | "dash" | "ksh")) => (path.to_string(), "-lc"),
            _ => ("/bin/sh".to_string(), "-c"),
        }
    }

    pub(super) fn run_command(
        command: &str,
        cwd: &str,
        timeout: Duration,
    ) -> Result<CommandResult, String> {
        let dir = Path::new(cwd);
        if !dir.is_absolute() {
            return Err(format!("Working directory must be absolute: {cwd}"));
        }
        if !dir.is_dir() {
            return Err(format!("Working directory does not exist: {cwd}"));
        }
        let (program, flag) = shell_for(std::env::var("SHELL").ok().as_deref());
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
        use std::os::unix::process::CommandExt;
        cmd.arg(flag).arg(command);
        // Own process group: the group id stays valid for a timeout kill even
        // after the shell itself has exited.
        cmd.process_group(0);
        cmd.current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Could not start {program}: {e}"))?;

        let stdout = child.stdout.take().map(TailReader::spawn);
        let stderr = child.stderr.take().map(TailReader::spawn);

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
        let (out, err) = collect([stdout, stderr], &mut child);
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

    /// A pipe reader running on its own thread. `tail` holds what has been read so
    /// far, so the output survives even when the thread never finishes.
    struct TailReader {
        tail: Arc<Mutex<Vec<u8>>>,
        done: mpsc::Receiver<()>,
    }

    impl TailReader {
        fn spawn<R: Read + Send + 'static>(mut stream: R) -> Self {
            let tail = Arc::new(Mutex::new(Vec::new()));
            let (tx, done) = mpsc::channel();
            let shared = Arc::clone(&tail);
            thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => break,
                        Ok(n) => {
                            let mut tail = shared.lock().unwrap_or_else(|e| e.into_inner());
                            tail.extend_from_slice(&chunk[..n]);
                            if tail.len() > OUTPUT_TAIL_BYTES * 2 {
                                let cut = tail.len() - OUTPUT_TAIL_BYTES;
                                tail.drain(..cut);
                            }
                        }
                    }
                }
                let _ = tx.send(());
            });
            Self { tail, done }
        }

        /// True once the pipe hit EOF, false if it was still open at `deadline`.
        fn finished_by(&self, deadline: Instant) -> bool {
            !matches!(
                self.done
                    .recv_timeout(deadline.saturating_duration_since(Instant::now())),
                Err(mpsc::RecvTimeoutError::Timeout)
            )
        }

        fn text(&self) -> String {
            let tail = self.tail.lock().unwrap_or_else(|e| e.into_inner());
            tail_chars(&tail, OUTPUT_TAIL_CHARS)
        }
    }

    /// Both waits are bounded, so a descendant that keeps a pipe open (even one
    /// that escaped the process group) cannot hold the call past the timeout. A
    /// reader that is still stuck is left detached and what it read so far is used.
    fn collect(readers: [Option<TailReader>; 2], child: &mut Child) -> (String, String) {
        let all_finished = |deadline: Instant| {
            // Check every reader (no short-circuit) so each gets its share of the wait.
            let states: Vec<bool> = readers
                .iter()
                .flatten()
                .map(|r| r.finished_by(deadline))
                .collect();
            states.iter().all(|&done| done)
        };
        if !all_finished(Instant::now() + READER_GRACE) {
            kill_tree(child);
            all_finished(Instant::now() + POST_KILL_WAIT);
        }
        let [out, err] = &readers;
        (
            out.as_ref().map(TailReader::text).unwrap_or_default(),
            err.as_ref().map(TailReader::text).unwrap_or_default(),
        )
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

    fn kill_tree(child: &mut Child) {
        // process_group(0) made the pid the group id; the negative pid reaches the
        // shell and every descendant, such as a `sleep` it started.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
    }

    #[cfg(test)]
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
        fn escaped_descendant_holding_the_pipe_does_not_block_the_timeout() {
            // perl's setsid moves the sleeper out of the process group, so the
            // group kill cannot reach it while it still holds the stdout pipe.
            if Command::new("perl").arg("-e").arg("1").status().is_err() {
                eprintln!("skipped: perl is not available to call setsid");
                return;
            }
            let started = Instant::now();
            let result = sh(
                "perl -MPOSIX -e 'POSIX::setsid(); sleep 5' & echo started; sleep 5",
                200,
            );
            assert!(result.timed_out);
            assert_eq!(result.exit_code, None);
            assert_eq!(result.stdout, "started\n");
            assert!(started.elapsed() < Duration::from_millis(2500));
        }

        #[test]
        fn signal_killed_child_has_no_exit_code() {
            let result = sh("kill -9 $$", 5000);
            assert_eq!(result.exit_code, None);
            assert!(!result.timed_out);
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
            let err = run_command("echo hi", "/no/such/dir/anywhere", Duration::from_secs(5))
                .unwrap_err();
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
}

#[cfg(windows)]
mod imp {
    use super::CommandResult;
    use std::time::Duration;

    pub(super) fn run_command(
        _command: &str,
        _cwd: &str,
        _timeout: Duration,
    ) -> Result<CommandResult, String> {
        Err("Command nodes are not supported on Windows yet".to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn refuses_every_command() {
            let err = run_command("echo hi", "C:\\", Duration::from_secs(1)).unwrap_err();
            assert_eq!(err, "Command nodes are not supported on Windows yet");
        }
    }
}
