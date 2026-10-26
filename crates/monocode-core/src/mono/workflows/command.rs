//! Run a workflow command node: one shell command in a working directory,
//! with a timeout that kills the whole process tree and a bounded output tail.
//! A leftover background process that inherited the output pipes is killed
//! after a short grace period; one that redirected its output is left running.
//!
//! A running command is registered under a caller-chosen id so it can be cancelled
//! (`cancel`), and with `crate::process` so quitting the app ends every one of them.
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
    /// Ended by `cancel`, not by itself or the timeout. On quit the app must call
    /// `cancel_all()` before `process::shutdown_all()` (wired in `quit` and `restart` in main.rs) to get this;
    /// without it a command killed by `shutdown_all` reports `cancelled: false`.
    pub cancelled: bool,
}

/// Runs a command and blocks until it ends; call it off the UI thread. `id` names it
/// for `cancel` while it runs.
pub fn run_command(
    id: &str,
    command: &str,
    cwd: &str,
    timeout_ms: u64,
) -> Result<CommandResult, String> {
    imp::run_command(id, command, cwd, Duration::from_millis(timeout_ms))
}

/// Kills the process group of the running command `id`. False when no such command runs.
/// The pending `run_command` call returns promptly with `cancelled: true`.
pub fn cancel(id: &str) -> bool {
    imp::cancel(id)
}

/// Kills every running command.
pub fn cancel_all() {
    imp::cancel_all();
}

#[cfg(unix)]
mod imp {
    use std::collections::HashMap;
    use std::io::{ErrorKind, Read};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex, OnceLock};
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
    /// How long a cancel for an id that is not running yet is remembered. The caller can
    /// cancel while its run request is still on the way here; the command then ends at once.
    const EARLY_CANCEL_TTL: Duration = Duration::from_secs(60);

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

    /// A running command: its process group and whether it was cancelled.
    struct Running {
        pgid: i32,
        cancelled: Arc<AtomicBool>,
    }

    #[derive(Default)]
    struct Registry {
        running: HashMap<String, Running>,
        /// Ids cancelled before they were running, and when.
        early_cancels: HashMap<String, Instant>,
    }

    impl Registry {
        fn forget_stale(&mut self, now: Instant) {
            self.early_cancels
                .retain(|_, at| now.saturating_duration_since(*at) < EARLY_CANCEL_TTL);
        }

        fn remember_early_cancel(&mut self, id: &str, now: Instant) {
            self.forget_stale(now);
            self.early_cancels.insert(id.to_string(), now);
        }

        /// True once, if `id` was cancelled before it ran and that is still remembered.
        fn take_early_cancel(&mut self, id: &str, now: Instant) -> bool {
            self.forget_stale(now);
            self.early_cancels.remove(id).is_some()
        }
    }

    fn registry() -> &'static Mutex<Registry> {
        static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
    }

    fn lock_registry() -> std::sync::MutexGuard<'static, Registry> {
        registry().lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keeps a command in the registry for as long as it lives; every exit path
    /// (timeout, error, panic) removes it again through `Drop`.
    struct Registration {
        id: String,
        cancelled: Arc<AtomicBool>,
    }

    impl Registration {
        fn new(id: &str, pgid: i32) -> Result<Self, String> {
            let mut registry = lock_registry();
            if registry.running.contains_key(id) {
                return Err(format!("A command with id {id} is already running"));
            }
            // Cancelled while its request was on the way: it is registered as cancelled,
            // and the caller kills the group before anything else.
            let cancelled = Arc::new(AtomicBool::new(
                registry.take_early_cancel(id, Instant::now()),
            ));
            registry.running.insert(
                id.to_string(),
                Running {
                    pgid,
                    cancelled: Arc::clone(&cancelled),
                },
            );
            Ok(Self {
                id: id.to_string(),
                cancelled,
            })
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            lock_registry().running.remove(&self.id);
        }
    }

    /// Kills the command's whole process group. False when `id` is not running; the
    /// cancel is then remembered for a minute, so the command ends at once if it starts.
    pub(super) fn cancel(id: &str) -> bool {
        let mut registry = lock_registry();
        let Some(entry) = registry.running.get(id) else {
            registry.remember_early_cancel(id, Instant::now());
            return false;
        };
        // Flagged before the kill, so the run call sees it once the child is gone.
        entry.cancelled.store(true, Ordering::SeqCst);
        kill_group(entry.pgid);
        true
    }

    pub(super) fn cancel_all() {
        cancel_where(|_| true);
    }

    /// Kills every running command whose id matches. Split out of `cancel_all` so a test
    /// can end only its own commands while other tests run beside it.
    fn cancel_where(matches: impl Fn(&str) -> bool) {
        for (id, entry) in lock_registry().running.iter() {
            if matches(id) {
                entry.cancelled.store(true, Ordering::SeqCst);
                kill_group(entry.pgid);
            }
        }
    }

    pub(super) fn run_command(
        id: &str,
        command: &str,
        cwd: &str,
        timeout: Duration,
    ) -> Result<CommandResult, String> {
        run_in(
            id,
            command,
            cwd,
            timeout,
            std::env::var("SHELL").ok().as_deref(),
        )
    }

    /// `run_command` with the login shell given, so a test never starts the user's own.
    fn run_in(
        id: &str,
        command: &str,
        cwd: &str,
        timeout: Duration,
        shell: Option<&str>,
    ) -> Result<CommandResult, String> {
        let dir = Path::new(cwd);
        if !dir.is_absolute() {
            return Err(format!("Working directory must be absolute: {cwd}"));
        }
        if !dir.is_dir() {
            return Err(format!("Working directory does not exist: {cwd}"));
        }
        let (program, flag) = shell_for(shell);
        execute(id, &program, flag, command, dir, timeout)
    }

    fn execute(
        id: &str,
        program: &str,
        flag: &str,
        command: &str,
        dir: &Path,
        timeout: Duration,
    ) -> Result<CommandResult, String> {
        if lock_registry().take_early_cancel(id, Instant::now()) {
            return Ok(CommandResult {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                cancelled: true,
            });
        }
        let mut cmd = Command::new(program);
        use std::os::unix::process::CommandExt;
        cmd.arg(flag).arg(command);
        // The ordinary signal mask (tokio and node children need SIGCHLD). Normal priority:
        // node timeouts are wall-clock, and IO throttling applies to the whole process.
        crate::process::reset_signal_mask(&mut cmd);
        // A Finder-launched app has launchd's bare PATH. Not in tests: the first call runs
        // the user's login shell, which reads their dotfiles.
        #[cfg(not(test))]
        crate::scm::shell::apply_gui_env(&mut cmd);
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
        let registration = match Registration::new(id, child.id() as i32) {
            Ok(registration) => registration,
            Err(e) => {
                kill_tree(&mut child);
                let _ = child.wait();
                return Err(e);
            }
        };
        // Quitting the app kills the group through `process::shutdown_all`, even when no
        // caller is left to cancel. Held until the readers are collected, with the group alive.
        let _quit_guard = crate::process::track(child.id());
        // Cancelled between the check above and the registration: nothing ran for long.
        if registration.cancelled.load(Ordering::SeqCst) {
            kill_tree(&mut child);
        }

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

        // The shell is gone, so the command no longer counts as running: leave the registry
        // now rather than after the readers are collected.
        let cancelled = registration.cancelled.load(Ordering::SeqCst) && !timed_out;
        drop(registration);

        // A descendant that outlives the shell can keep a pipe open forever; give
        // the readers a moment, then end the group so the join cannot hang.
        let (out, err) = collect([stdout, stderr], &mut child);
        Ok(CommandResult {
            exit_code: if timed_out || cancelled {
                None
            } else {
                status.and_then(|s| s.code())
            },
            stdout: out,
            stderr: err,
            timed_out,
            cancelled,
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

    fn kill_group(pgid: i32) {
        // process_group(0) made the pid the group id; the negative pid reaches the
        // shell and every descendant, such as a `sleep` it started.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }

    fn kill_tree(child: &mut Child) {
        kill_group(child.id() as i32);
        let _ = child.kill();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        use std::sync::atomic::AtomicUsize;

        /// A fresh id per call, so tests running in parallel never share a registry slot.
        fn unique_id() -> String {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            format!("test-{}", NEXT.fetch_add(1, Ordering::SeqCst))
        }

        fn sh(command: &str, timeout_ms: u64) -> CommandResult {
            execute(
                &unique_id(),
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
            assert!(!result.cancelled);
        }

        #[test]
        fn the_command_keeps_the_callers_priority() {
            let ours = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
            if ours >= 10 {
                eprintln!("skipped: the test already runs at background priority");
                return;
            }
            let result = sh("ps -o ni= -p $$", 5000);
            assert_eq!(result.stdout.trim().parse::<i32>().ok(), Some(ours));
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
            let err = run_command(
                &unique_id(),
                "echo hi",
                "relative/dir",
                Duration::from_secs(5),
            )
            .unwrap_err();
            assert!(err.contains("absolute"), "{err}");
        }

        #[test]
        fn rejects_missing_cwd() {
            let err = run_command(
                &unique_id(),
                "echo hi",
                "/no/such/dir/anywhere",
                Duration::from_secs(5),
            )
            .unwrap_err();
            assert!(err.contains("does not exist"), "{err}");
        }

        #[test]
        fn runs_in_the_given_cwd() {
            let dir = std::env::temp_dir().canonicalize().expect("temp dir");
            let result = run_in(
                &unique_id(),
                "pwd -P",
                dir.to_str().unwrap(),
                Duration::from_secs(10),
                None,
            )
            .expect("command runs");
            assert_eq!(result.exit_code, Some(0));
            assert!(result.stdout.trim_end().ends_with(dir.to_str().unwrap()));
        }

        /// Runs `command` on a thread and waits until it shows up in the registry.
        fn start(id: &str, command: &str) -> thread::JoinHandle<CommandResult> {
            let (id_owned, command) = (id.to_string(), command.to_string());
            let handle = thread::spawn(move || {
                execute(
                    &id_owned,
                    "/bin/sh",
                    "-c",
                    &command,
                    Path::new("/tmp"),
                    Duration::from_secs(60),
                )
                .expect("command runs")
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while !lock_registry().running.contains_key(id) {
                assert!(Instant::now() < deadline, "{id} never registered");
                thread::sleep(Duration::from_millis(5));
            }
            handle
        }

        #[test]
        fn cancel_kills_the_running_group_and_flags_the_result() {
            let id = unique_id();
            let handle = start(&id, "sleep 30 & echo $!; wait");
            // Registered means spawned; give the shell time to print the grandchild's pid.
            thread::sleep(Duration::from_millis(300));
            let started = Instant::now();
            assert!(cancel(&id));
            let result = handle.join().expect("run thread");
            assert!(started.elapsed() < Duration::from_secs(3));
            assert!(result.cancelled);
            assert!(!result.timed_out);
            assert_eq!(result.exit_code, None);
            let pid: i32 = result.stdout.trim().parse().expect("grandchild pid");
            let mut gone = false;
            for _ in 0..50 {
                if unsafe { libc::kill(pid, 0) } != 0 {
                    gone = true;
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            assert!(gone, "grandchild {pid} survived the cancel");
            assert!(!lock_registry().running.contains_key(&id));
        }

        #[test]
        fn cancel_of_an_unknown_id_is_false() {
            assert!(!cancel("no-such-command"));
        }

        #[test]
        fn registry_is_empty_after_every_exit_path() {
            let done = unique_id();
            let timed = unique_id();
            let exited = unique_id();
            let ids = [done.clone(), timed.clone(), exited.clone()];
            let run = |id: &str, command: &str, timeout_ms: u64| {
                execute(
                    id,
                    "/bin/sh",
                    "-c",
                    command,
                    Path::new("/tmp"),
                    Duration::from_millis(timeout_ms),
                )
                .expect("command runs")
            };
            run(&done, "echo hi", 5000);
            assert!(run(&timed, "sleep 5", 100).timed_out);
            run(&exited, "exit 4", 5000);
            let registry = lock_registry();
            assert!(ids.iter().all(|id| !registry.running.contains_key(id)));
            // A finished command cannot be cancelled afterwards.
            drop(registry);
            assert!(!cancel(&done));
        }

        #[test]
        fn a_spawn_failure_leaves_nothing_registered() {
            let id = unique_id();
            let err = execute(
                &id,
                "/no/such/shell",
                "-c",
                "true",
                Path::new("/tmp"),
                Duration::from_secs(1),
            )
            .unwrap_err();
            assert!(err.contains("Could not start"), "{err}");
            assert!(!lock_registry().running.contains_key(&id));
        }

        #[test]
        fn a_duplicate_id_is_refused_and_leaves_the_first_running() {
            let id = unique_id();
            let first = start(&id, "sleep 30");
            let err = execute(
                &id,
                "/bin/sh",
                "-c",
                "echo second",
                Path::new("/tmp"),
                Duration::from_secs(5),
            )
            .unwrap_err();
            assert!(err.contains("already running"), "{err}");
            assert!(cancel(&id));
            assert!(first.join().expect("run thread").cancelled);
        }

        #[test]
        fn a_cancel_that_came_first_ends_the_command_before_it_runs() {
            let id = unique_id();
            assert!(!cancel(&id));
            let started = Instant::now();
            let result = execute(
                &id,
                "/bin/sh",
                "-c",
                "echo ran; sleep 30",
                Path::new("/tmp"),
                Duration::from_secs(60),
            )
            .expect("call returns");
            assert!(started.elapsed() < Duration::from_secs(1));
            assert!(result.cancelled);
            assert!(!result.timed_out);
            assert_eq!(result.exit_code, None);
            assert_eq!(result.stdout, "");
            // The memory of the cancel is used up: the same id can run normally later.
            let again = execute(
                &id,
                "/bin/sh",
                "-c",
                "echo ran",
                Path::new("/tmp"),
                Duration::from_secs(5),
            )
            .expect("command runs");
            assert!(!again.cancelled);
            assert_eq!(again.stdout, "ran\n");
        }

        #[test]
        fn a_cancel_that_lands_during_registration_marks_the_command_cancelled() {
            let id = unique_id();
            assert!(!cancel(&id));
            let registration = Registration::new(&id, 0).expect("registers");
            assert!(registration.cancelled.load(Ordering::SeqCst));
            drop(registration);
            let fresh = Registration::new(&id, 0).expect("registers");
            assert!(!fresh.cancelled.load(Ordering::SeqCst));
        }

        #[test]
        fn an_early_cancel_is_forgotten_after_its_ttl() {
            let mut registry = Registry::default();
            let now = Instant::now();
            registry.remember_early_cancel("a", now);
            registry.remember_early_cancel("b", now);
            assert!(registry.take_early_cancel("a", now + EARLY_CANCEL_TTL / 2));
            // Taken once only.
            assert!(!registry.take_early_cancel("a", now + EARLY_CANCEL_TTL / 2));
            assert!(
                !registry.take_early_cancel("b", now + EARLY_CANCEL_TTL + Duration::from_secs(1))
            );
            assert!(registry.early_cancels.is_empty());
        }

        #[test]
        fn the_registry_is_left_before_the_readers_are_collected() {
            // A descendant keeps the pipe open past the shell, so collect() waits; the
            // command must already be gone from the registry by then.
            let id = unique_id();
            let handle = start(&id, "sleep 3 & echo started");
            let deadline = Instant::now() + Duration::from_secs(2);
            while lock_registry().running.contains_key(&id) {
                assert!(Instant::now() < deadline, "still registered");
                thread::sleep(Duration::from_millis(5));
            }
            assert!(!handle.is_finished(), "collect() should still be waiting");
            let result = handle.join().expect("run thread");
            assert_eq!(result.exit_code, Some(0));
        }

        #[test]
        fn cancel_all_kills_every_running_command() {
            let ids = [
                format!("all-{}", unique_id()),
                format!("all-{}", unique_id()),
            ];
            let handles: Vec<_> = ids.iter().map(|id| start(id, "sleep 30")).collect();
            let started = Instant::now();
            cancel_where(|id| id.starts_with("all-"));
            for handle in handles {
                let result = handle.join().expect("run thread");
                assert!(result.cancelled);
                assert!(!result.timed_out);
            }
            assert!(started.elapsed() < Duration::from_secs(3));
            let registry = lock_registry();
            assert!(ids.iter().all(|id| !registry.running.contains_key(id)));
        }

        /// `process::shutdown_all` kills every tracked process of the test binary, so it runs
        /// in a child copy of the binary and cannot touch the commands of other tests.
        #[test]
        fn shutdown_all_kills_a_running_command() {
            let exe = std::env::current_exe().expect("test binary");
            let output = Command::new(exe)
                .args(["--ignored", "--exact", "--nocapture", "--test-threads=1"])
                .arg(format!(
                    "{}::shutdown_all_child",
                    // The test name has no crate prefix.
                    module_path!().split_once("::").map_or("", |(_, rest)| rest)
                ))
                .output()
                .expect("child runs");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{stdout}");
            assert!(
                stdout.contains("shutdown-child-ok"),
                "child did not run: {stdout}"
            );
        }

        /// The body of the test above; only run by it, in a child process.
        #[test]
        #[ignore = "run by shutdown_all_kills_a_running_command in a child process"]
        fn shutdown_all_child() {
            let id = unique_id();
            let handle = start(&id, "sleep 30 & echo $!; wait");
            thread::sleep(Duration::from_millis(300));
            let started = Instant::now();
            crate::process::shutdown_all();
            let result = handle.join().expect("run thread");
            assert!(started.elapsed() < Duration::from_secs(3));
            // Killed by the signal, not by the timeout or a cancel.
            assert_eq!(result.exit_code, None);
            assert!(!result.timed_out);
            let pid: i32 = result.stdout.trim().parse().expect("grandchild pid");
            let gone = (0..50).any(|_| {
                if unsafe { libc::kill(pid, 0) } != 0 {
                    return true;
                }
                thread::sleep(Duration::from_millis(20));
                false
            });
            assert!(gone, "grandchild {pid} survived shutdown_all");
            println!("shutdown-child-ok");
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
        _id: &str,
        _command: &str,
        _cwd: &str,
        _timeout: Duration,
    ) -> Result<CommandResult, String> {
        Err("Command nodes are not supported on Windows yet".to_string())
    }

    /// Nothing runs on Windows, so there is nothing to cancel.
    pub(super) fn cancel(_id: &str) -> bool {
        false
    }

    pub(super) fn cancel_all() {}

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn refuses_every_command() {
            let err = run_command("id", "echo hi", "C:\\", Duration::from_secs(1)).unwrap_err();
            assert_eq!(err, "Command nodes are not supported on Windows yet");
        }

        #[test]
        fn nothing_can_be_cancelled() {
            assert!(!cancel("id"));
            cancel_all();
        }
    }
}
