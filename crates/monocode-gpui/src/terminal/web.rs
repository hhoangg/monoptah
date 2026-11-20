//! The shell behind the web terminal: a real pseudo-terminal started the way
//! the terminal dock starts one (alacritty's `tty`), with its output pumped
//! into the core's bounded [`Output`] window and its whole process group
//! killed on request. The core decides who may open one and when it ends;
//! this only runs and ends the process.
use monocode_core::mobile::terminal::{Output, ShellHandle, ShellSpawner, SpawnSpec};
use std::sync::Arc;

/// Starts the user's login shell on a pseudo-terminal. Unix only for now: on
/// other systems opening answers with a plain error.
pub(crate) struct WebShellSpawner;

impl ShellSpawner for WebShellSpawner {
    fn spawn(&self, spec: SpawnSpec, out: Arc<Output>) -> Result<Arc<dyn ShellHandle>, String> {
        #[cfg(unix)]
        {
            let (program, args) =
                super::shell::default_shell(&monocode_core::scm::terminal_search_path());
            unix::spawn(spec, out, program, args)
        }
        #[cfg(not(unix))]
        {
            let _ = (spec, out);
            Err("The web terminal is not available on this system yet".into())
        }
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use alacritty_terminal::{event::WindowSize, tty};
    use std::{
        collections::HashMap,
        fs::File,
        io::{ErrorKind, Read, Write},
        os::fd::AsRawFd,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    /// Bytes read from the pseudo-terminal at once.
    pub(super) const CHUNK: usize = 16 * 1024;
    /// How long a keystroke write may wait for a full pseudo-terminal.
    const WRITE_WAIT: Duration = Duration::from_secs(2);
    /// Time between SIGHUP and SIGKILL of the shell's processes.
    const HANGUP_GRACE: Duration = Duration::from_millis(500);

    static WINDOW_IDS: AtomicU64 = AtomicU64::new(1);

    struct Shell {
        /// The shell, and the leader of its process group (it called `setsid`).
        pid: i32,
        writer: Mutex<File>,
        /// Dropped on a helper thread after the kill: its `Drop` waits for the child.
        pty: Mutex<Option<tty::Pty>>,
        out: Arc<Output>,
        killed: AtomicBool,
    }

    pub(super) fn spawn(
        spec: SpawnSpec,
        out: Arc<Output>,
        program: String,
        args: Vec<String>,
    ) -> Result<Arc<dyn ShellHandle>, String> {
        let mut env = HashMap::new();
        for (key, value) in [
            ("TERM", "xterm-256color".to_owned()),
            ("COLORTERM", "truecolor".to_owned()),
            ("TERM_PROGRAM", "Monoptah".to_owned()),
            ("PATH", monocode_core::scm::terminal_search_path()),
            ("PWD", spec.cwd.to_string_lossy().into_owned()),
        ] {
            env.insert(key.to_owned(), value);
        }
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, args)),
            working_directory: Some(spec.cwd),
            drain_on_exit: false,
            env,
        };
        let window = WindowSize {
            num_lines: spec.rows,
            num_cols: spec.cols,
            cell_width: 8,
            cell_height: 16,
        };
        let pty = tty::new(&options, window, WINDOW_IDS.fetch_add(1, Ordering::Relaxed))
            .map_err(|error| format!("Could not start the shell: {error}"))?;
        let pid = i32::try_from(pty.child().id()).map_err(|_| "Invalid shell process")?;
        let reader = pty.file().try_clone().map_err(|e| e.to_string())?;
        let writer = pty.file().try_clone().map_err(|e| e.to_string())?;
        let shell = Arc::new(Shell {
            pid,
            writer: Mutex::new(writer),
            pty: Mutex::new(Some(pty)),
            out: out.clone(),
            killed: AtomicBool::new(false),
        });
        let pumped = shell.clone();
        std::thread::Builder::new()
            .name("web-terminal-pump".into())
            .spawn(move || pump(reader, &pumped.out))
            .map_err(|e| e.to_string())?;
        Ok(shell)
    }

    /// Copies the shell's output into the window until the shell is gone or
    /// the window is closed. The file is non-blocking: an idle shell is polled
    /// with a growing pause, so it costs next to nothing.
    fn pump(mut reader: File, out: &Output) {
        let mut buffer = vec![0u8; CHUNK];
        let mut pause = Duration::from_millis(2);
        while !out.is_closed() {
            match reader.read(&mut buffer) {
                // The slave side is closed: Linux reports EIO, macOS an EOF.
                Ok(0) => break,
                Ok(n) => {
                    pause = Duration::from_millis(2);
                    // Blocks while the browser is behind: the shell slows down.
                    if !out.push(&buffer[..n]) {
                        return;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(pause);
                    pause = (pause * 2).min(Duration::from_millis(25));
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        if !out.is_closed() {
            out.finish(None);
        }
    }

    impl Shell {
        /// The pseudo-terminal's foreground process group, if it has one.
        fn foreground_group(&self) -> Option<i32> {
            let writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            foreground_of(writer.as_raw_fd())
        }
    }

    impl ShellHandle for Shell {
        fn write(&self, data: &[u8]) -> Result<(), String> {
            let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            let started = Instant::now();
            let mut rest = data;
            while !rest.is_empty() {
                match writer.write(rest) {
                    Ok(0) => return Err("The shell closed its input".into()),
                    Ok(n) => rest = &rest[n..],
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        if started.elapsed() > WRITE_WAIT {
                            return Err("The shell is not reading its input".into());
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(())
        }

        fn resize(&self, cols: u16, rows: u16) {
            let writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            let size = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            // SAFETY: a valid pseudo-terminal master and a fully initialised struct.
            unsafe {
                libc::ioctl(writer.as_raw_fd(), libc::TIOCSWINSZ, &size);
            }
        }

        /// SIGHUP to the whole group at once, SIGKILL to it half a second
        /// later from a helper thread, which then drops the pseudo-terminal
        /// (that waits for the child, so it must never run on the caller).
        fn kill(&self) {
            if self.killed.swap(true, Ordering::SeqCst) {
                return;
            }
            self.out.close();
            let foreground = self.foreground_group();
            signal_all(self.pid, foreground, libc::SIGHUP);
            let pid = self.pid;
            let pty = self.pty.lock().unwrap_or_else(|e| e.into_inner()).take();
            let _ = std::thread::Builder::new()
                .name("web-terminal-reap".into())
                .spawn(move || {
                    std::thread::sleep(HANGUP_GRACE);
                    // The shell is still unreaped (only the drop below waits for
                    // it), so its pid cannot have been handed to anything else.
                    // Looked up again: the pty is still open, and jobs started
                    // since the hangup are in the session too.
                    let foreground = pty
                        .as_ref()
                        .and_then(|pty| foreground_of(pty.file().as_raw_fd()));
                    signal_all(pid, foreground, libc::SIGKILL);
                    drop(pty);
                });
        }
    }

    fn foreground_of(fd: i32) -> Option<i32> {
        // SAFETY: a plain query on an open pseudo-terminal master.
        let group = unsafe { libc::tcgetpgrp(fd) };
        (group > 1).then_some(group)
    }

    /// Signals the shell's group, the terminal's foreground group and every
    /// process whose session is the shell's. An interactive shell gives each
    /// job its own group, so the session is what finds `nohup job &`. A
    /// process that called `setsid` itself (a daemon) leaves the session and
    /// is out of reach: this is a kill, not containment.
    fn signal_all(pid: i32, foreground: Option<i32>, signal: i32) {
        // SAFETY: plain signals to groups and processes of the shell's session.
        unsafe {
            libc::killpg(pid, signal);
            if let Some(group) = foreground {
                libc::killpg(group, signal);
            }
            for member in session_members(pid) {
                libc::kill(member, signal);
            }
            libc::kill(pid, signal);
        }
    }

    #[cfg(target_os = "linux")]
    fn all_pids() -> Vec<i32> {
        std::fs::read_dir("/proc")
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(target_os = "macos")]
    fn all_pids() -> Vec<i32> {
        // SAFETY: a null buffer asks for the size; the second call fills a
        // buffer of exactly that many pids.
        unsafe {
            let bytes = libc::proc_listallpids(std::ptr::null_mut(), 0);
            if bytes <= 0 {
                return Vec::new();
            }
            let mut pids = vec![0i32; bytes as usize / std::mem::size_of::<i32>() + 64];
            let size = (pids.len() * std::mem::size_of::<i32>()) as libc::c_int;
            let filled = libc::proc_listallpids(pids.as_mut_ptr().cast(), size);
            if filled <= 0 {
                return Vec::new();
            }
            pids.truncate(filled as usize / std::mem::size_of::<i32>());
            pids.retain(|pid| *pid > 0);
            pids
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn all_pids() -> Vec<i32> {
        Vec::new()
    }

    /// Every other process in the session the shell leads.
    fn session_members(session: i32) -> Vec<i32> {
        all_pids()
            .into_iter()
            // SAFETY: `getsid` only reads the process table.
            .filter(|pid| *pid != session && unsafe { libc::getsid(*pid) } == session)
            .collect()
    }

    #[cfg(test)]
    pub(super) mod tests {
        use super::*;

        pub(in super::super) fn start(
            out: &Arc<Output>,
            program: &str,
            args: &[&str],
        ) -> Arc<dyn ShellHandle> {
            let dir = std::env::temp_dir();
            spawn(
                SpawnSpec {
                    cwd: dir,
                    cols: 80,
                    rows: 24,
                },
                out.clone(),
                program.to_owned(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
            )
            .expect("a shell starts")
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::unix::{CHUNK, tests::start};
    use super::*;
    use std::time::{Duration, Instant};

    fn read_until(out: &Output, needle: &str, wait: Duration) -> String {
        let started = Instant::now();
        let (mut from, mut text) = (0, String::new());
        while started.elapsed() < wait && !text.contains(needle) {
            let taken = out.take(from, 4096);
            from = taken.next;
            text.push_str(&String::from_utf8_lossy(&taken.data));
            std::thread::sleep(Duration::from_millis(10));
        }
        text
    }

    /// A process that is still running (a zombie nobody reaped is not).
    fn alive(pid: i32) -> bool {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        let state = String::from_utf8_lossy(&output.stdout);
        !state.trim().is_empty() && !state.trim().starts_with('Z')
    }

    #[test]
    fn a_real_shell_takes_input_and_streams_its_output() {
        let out = Output::new(64 * 1024);
        let shell = start(&out, "/bin/sh", &[]);
        shell.write(b"echo web-terminal-$((20+22))\n").unwrap();
        let text = read_until(&out, "web-terminal-42", Duration::from_secs(10));
        assert!(text.contains("web-terminal-42"), "{text:?}");
        shell.resize(100, 30);
        shell.write(b"stty size\n").unwrap();
        let text = read_until(&out, "30 100", Duration::from_secs(10));
        assert!(text.contains("30 100"), "{text:?}");
        shell.kill();
    }

    #[test]
    fn killing_ends_the_shell_and_every_process_in_its_group() {
        let out = Output::new(64 * 1024);
        // Not interactive, so the background job stays in the shell's group.
        let shell = start(&out, "/bin/sh", &["-c", "sleep 300 & echo job=$!; wait"]);
        let text = read_until(&out, "\n", Duration::from_secs(10));
        let pid: i32 = text
            .split("job=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|digits| digits.parse().ok())
            .unwrap_or_else(|| panic!("no job pid in {text:?}"));
        assert!(alive(pid), "the background job runs before the kill");
        let started = Instant::now();
        shell.kill();
        shell.kill();
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "kill does not wait"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(pid), "the background job died with the group");
        assert!(out.is_closed());
    }

    #[test]
    fn killing_reaches_a_nohup_job_in_its_own_process_group() {
        let out = Output::new(64 * 1024);
        // Interactive with job control: the job gets its own group, so a
        // plain group kill would miss it.
        let shell = start(&out, "/bin/bash", &["--norc", "--noprofile", "-i"]);
        // The marker is built from `$((0))`, so the typed line the pty echoes
        // back cannot match it: only the shell's real answer does.
        shell
            .write(b"set -m; nohup sleep 1000 >/dev/null 2>&1 & echo JOB:$((0))$!\n")
            .unwrap();
        let started = Instant::now();
        let mut seen = String::new();
        let mut from = 0;
        let pid: i32 = loop {
            let taken = out.take(from, 4096);
            from = taken.next;
            seen.push_str(&String::from_utf8_lossy(&taken.data));
            let found = seen.match_indices("JOB:").find_map(|(at, _)| {
                let rest = &seen[at + 4..];
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                (!digits.is_empty() && rest[digits.len()..].starts_with('\r'))
                    .then(|| digits.parse().ok())
                    .flatten()
            });
            if let Some(pid) = found {
                break pid;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "no job pid in {seen:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(alive(pid), "the job runs before the kill");
        shell.kill();
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(pid), "the nohup job died with the session");
    }

    #[test]
    fn a_shell_that_ignores_hangup_is_killed_anyway() {
        let out = Output::new(64 * 1024);
        let shell = start(
            &out,
            "/bin/sh",
            &[
                "-c",
                "trap '' HUP; echo ready; while true; do sleep 1; done",
            ],
        );
        assert!(read_until(&out, "ready", Duration::from_secs(10)).contains("ready"));
        let pid = {
            // The shell is the first process the spawner made.
            let text = std::process::Command::new("pgrep")
                .args(["-f", "trap '' HUP; echo ready"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&text.stdout)
                .split_whitespace()
                .next()
                .and_then(|pid| pid.parse::<i32>().ok())
                .expect("the shell has a pid")
        };
        shell.kill();
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(pid), "SIGKILL follows the ignored SIGHUP");
    }

    #[test]
    fn a_shell_that_exits_by_itself_finishes_the_output() {
        let out = Output::new(64 * 1024);
        let _shell = start(&out, "/bin/sh", &["-c", "echo bye"]);
        let started = Instant::now();
        loop {
            let taken = out.take(0, 4096);
            if taken.exit.is_some() {
                assert!(String::from_utf8_lossy(&taken.data).contains("bye"));
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the end was not noticed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn heavy_output_waits_for_the_browser_instead_of_growing_the_buffer() {
        let window = 4096;
        let out = Output::new(window);
        let shell = start(
            &out,
            "/bin/sh",
            &["-c", "head -c 600000 /dev/zero | tr '\\0' a; echo finished"],
        );
        // Nobody reads: the pump stops at the window (plus one chunk).
        std::thread::sleep(Duration::from_millis(800));
        assert!(
            out.buffered() <= window + CHUNK,
            "buffered {}",
            out.buffered()
        );
        // Reading with acknowledgements takes every byte, in order.
        let (mut from, mut total, mut tail) = (0u64, 0usize, String::new());
        let started = Instant::now();
        while !tail.contains("finished") {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "stalled at {total}"
            );
            let taken = out.take(from, 8192);
            from = taken.next;
            total += taken.data.len();
            tail.push_str(&String::from_utf8_lossy(&taken.data));
            tail = tail[tail.len().saturating_sub(64)..].to_owned();
            if taken.data.is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(total >= 600_000, "{total}");
        shell.kill();
    }
}
