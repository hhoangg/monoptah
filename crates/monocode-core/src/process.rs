//! Child policy extracted from src-tauri/src/background_work.rs.
use std::process::Command;

pub mod guard;

/// Give the child the ordinary process mask. A GUI executor's worker may block
/// signals and exec preserves that mask; Tokio-based children then never receive
/// SIGCHLD and their command sessions never finish.
pub fn reset_signal_mask(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Only system calls here: the post-fork child must not allocate or lock.
        unsafe {
            command.pre_exec(|| {
                let mut signals = std::mem::zeroed();
                libc::sigemptyset(&mut signals);
                if libc::sigprocmask(libc::SIG_SETMASK, &signals, std::ptr::null_mut()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let _ = command;
}

/// Apply scheduling hints before exec so descendants inherit them.
/// Best effort: a refused hint must not prevent a provider from launching.
pub fn configure_child(command: &mut Command) {
    reset_signal_mask(command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Only system calls here: the post-fork child must not allocate or lock.
        unsafe {
            command.pre_exec(|| {
                let priority = libc::getpriority(libc::PRIO_PROCESS, 0).max(10);
                libc::setpriority(libc::PRIO_PROCESS, 0, priority);
                #[cfg(target_os = "macos")]
                setiopolicy_np(0, 0, 3); // DISK, PROCESS, THROTTLE
                Ok(())
            });
        }
    }
    // A GUI-subsystem parent gives every console child (git, gh) its own
    // visible console window unless asked not to.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = command;
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn setiopolicy_np(kind: i32, scope: i32, policy: i32) -> i32;
}

/// New native provider processes use a separate group for cancellation.
pub fn isolate(command: &mut Command) {
    configure_child(command);
    command.env("MONOCODE_HARNESS_PARENT", std::process::id().to_string());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW | 0x0000_0200);
    }
}

/// Called only by the provider worker. Always reap the immediate child.
pub fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let mut kill = Command::new("taskkill");
        kill.args(["/PID", &child.id().to_string(), "/T", "/F"]);
        use std::os::windows::process::CommandExt;
        kill.creation_flags(CREATE_NO_WINDOW);
        let _ = kill.status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// How long a provider whose stdin was closed may take to exit by itself.
#[cfg(unix)]
const EXIT_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);
/// How long the group leader may take to shut down after SIGTERM.
#[cfg(unix)]
const TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Stop a provider child without cutting off a credential write, on a
/// background thread. Close its stdin before calling this.
///
/// Claude Code rotates its OAuth refresh token on every refresh and saves the
/// new one through a `security -i` child. Killing the whole group in that
/// window loses the new token and leaves Keychain holding a spent one, which
/// signs the account out. Close stdin first, then send SIGTERM to the leader
/// only (Claude shuts down cleanly on it), and SIGKILL the group last.
pub fn retire(mut child: std::process::Child) {
    drop(child.stdin.take());
    #[cfg(unix)]
    {
        let pid = child.id();
        // std never reaps a dropped Child; `retire_pid` waits for it below.
        drop(child);
        std::thread::spawn(move || retire_pid(pid, || {}));
    }
    #[cfg(not(unix))]
    std::thread::spawn(move || terminate(&mut child));
}

/// Blocking form of [`retire`] for a child whose `Child` value cannot be
/// moved. `after` runs once the process is gone (to join pipe readers).
#[cfg(unix)]
pub(crate) fn retire_pid(pid: u32, after: impl FnOnce()) {
    let pid = pid as libc::pid_t;
    let exited = |within: std::time::Duration| {
        let deadline = std::time::Instant::now() + within;
        loop {
            let mut status = 0;
            match unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } {
                0 if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                0 => return false,
                // Reaped now, or already reaped / not our child.
                _ => return true,
            }
        }
    };
    if !exited(EXIT_GRACE) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        if !exited(TERM_GRACE) {
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            let mut status = 0;
            unsafe { libc::waitpid(pid, &mut status, 0) };
        }
    }
    // The leader has finished its own writes. Clear any helpers it left behind.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    after();
}

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
/// A provider child that belongs to a session, for the running-work list.
#[derive(Clone, Debug)]
pub struct Agent {
    pub session: i64,
    pub pid: u32,
    /// Birth stamp at spawn; a reused PID no longer matches it.
    pub identity: Option<String>,
}
/// A registered child. `session` survives going idle so an app session can
/// still find the warm process it owns; `agent` is only set while it works.
struct Slot {
    session: Option<i64>,
    agent: Option<Agent>,
}
fn active() -> &'static Mutex<HashMap<u32, Slot>> {
    static ACTIVE: OnceLock<Mutex<HashMap<u32, Slot>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}
thread_local! {
    static OWNER: Cell<Option<i64>> = const { Cell::new(None) };
}
/// Children spawned by `run` on this thread are attributed to `session`.
pub fn owned_by<T>(session: i64, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<i64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OWNER.set(self.0);
        }
    }
    let _restore = Restore(OWNER.replace(Some(session)));
    run()
}
/// Live provider children that belong to a session.
pub fn agents() -> Vec<Agent> {
    active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .filter_map(|slot| slot.agent.clone())
        .collect()
}
pub(crate) fn owner() -> Option<i64> {
    OWNER.get()
}
pub(crate) fn mark_idle(pid: u32, idle: bool) {
    let mut processes = active().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(slot) = processes.get_mut(&pid) {
        if idle {
            slot.agent = None;
        } else {
            if let Some(session) = OWNER.get() {
                slot.session = Some(session);
            }
            slot.agent = slot.session.map(|session| Agent {
                session,
                pid,
                identity: crate::resources::identity(pid),
            });
        }
    }
}
/// Keeps a pid in the registry `shutdown_all` reaps; unregisters on drop.
pub struct Registration(u32);
/// Register a child spawned elsewhere (a terminal session's CLI) so quitting
/// kills its process group. Hold the value for as long as the child lives.
pub fn track(pid: u32) -> Registration {
    register(pid)
}
pub(crate) fn register(pid: u32) -> Registration {
    let agent = OWNER.get().map(|session| Agent {
        session,
        pid,
        identity: crate::resources::identity(pid),
    });
    active().lock().unwrap_or_else(|e| e.into_inner()).insert(
        pid,
        Slot {
            session: OWNER.get(),
            agent,
        },
    );
    Registration(pid)
}
impl Drop for Registration {
    fn drop(&mut self) {
        active()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}
static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// True once `shutdown_all` ran: nothing may park a process for reuse after it.
pub(crate) fn quitting() -> bool {
    QUITTING.load(std::sync::atomic::Ordering::SeqCst)
}

fn epochs() -> &'static Mutex<HashMap<i64, u64>> {
    static EPOCHS: OnceLock<Mutex<HashMap<i64, u64>>> = OnceLock::new();
    EPOCHS.get_or_init(|| Mutex::new(HashMap::new()))
}
/// Generation of an app session's process lifecycle. A process records it when
/// it starts or is taken for a turn and may be parked for reuse only while it
/// is unchanged; [`crate::provider::retire_session`] bumps it.
pub(crate) fn session_epoch(session: Option<i64>) -> u64 {
    session.map_or(0, |session| {
        epochs()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&session)
            .copied()
            .unwrap_or(0)
    })
}
thread_local! {
    static TURN_EPOCH: Cell<Option<u64>> = const { Cell::new(None) };
}
/// Ends the turn scope opened by [`begin_turn`].
pub(crate) struct TurnScope(Option<u64>);
impl Drop for TurnScope {
    fn drop(&mut self) {
        TURN_EPOCH.set(self.0);
    }
}
/// Record the owner's epoch at the very start of a turn, before anything is
/// spawned, so a spawn that began before an archive can never be parked.
pub(crate) fn begin_turn() -> TurnScope {
    TurnScope(TURN_EPOCH.replace(Some(session_epoch(OWNER.get()))))
}
/// The epoch a process started or taken by this turn is recorded with.
pub(crate) fn turn_epoch() -> u64 {
    TURN_EPOCH
        .get()
        .unwrap_or_else(|| session_epoch(OWNER.get()))
}
/// True while a process recorded at `epoch` for the current owner may be parked.
pub(crate) fn may_park(epoch: u64) -> bool {
    !quitting() && session_epoch(OWNER.get()) == epoch
}

/// Pids still registered to `session` (running or warm).
pub(crate) fn session_pids(session: i64) -> Vec<u32> {
    active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, slot)| slot.session == Some(session))
        .map(|(pid, _)| *pid)
        .collect()
}
fn is_registered(pid: u32) -> bool {
    active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&pid)
}

/// Invalidate every process of `session` for parking. Call before sweeping the
/// pools.
pub(crate) fn bump_epoch(session: i64) {
    *epochs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(session)
        .or_insert(0) += 1;
}

/// Terminate the `snapshot` pids (taken from `session_pids` right after
/// `bump_epoch`, so a later turn's process is never in it) on a background
/// thread, after the pool sweeps.
/// `already_retiring` are pids whose owner (a dropped pool entry) retires them.
///
/// Unix: SIGTERM to the leader, a short grace, then SIGKILL to the whole
/// process group (MCP helpers such as `visual-mcp` share it). Only pids still
/// in the registry are signalled (our own children, not yet released by their
/// owner); a tiny window between reaping and unregistering remains. Windows:
/// `taskkill /T /F` kills the tree. Elsewhere: no-op.
pub(crate) fn retire_session_processes(snapshot: Vec<u32>, already_retiring: Vec<u32>) {
    let pids: Vec<u32> = snapshot
        .into_iter()
        .filter(|pid| !already_retiring.contains(pid))
        .collect();
    if pids.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        #[cfg(unix)]
        {
            for pid in &pids {
                if is_registered(*pid) {
                    unsafe { libc::kill(*pid as i32, libc::SIGTERM) };
                }
            }
            let deadline = std::time::Instant::now() + TERM_GRACE;
            while pids.iter().any(|pid| is_registered(*pid)) && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // The leader may be gone while helpers live on: its owner clears
            // the group when it reaps it, so only still-registered groups are hit.
            for pid in &pids {
                if is_registered(*pid) {
                    unsafe { libc::kill(-(*pid as i32), libc::SIGKILL) };
                }
            }
        }
        #[cfg(windows)]
        for pid in &pids {
            if is_registered(*pid) {
                let mut c = Command::new("taskkill");
                c.args(["/PID", &pid.to_string(), "/T", "/F"]);
                use std::os::windows::process::CommandExt;
                c.creation_flags(CREATE_NO_WINDOW);
                let _ = c.status();
            }
        }
        #[cfg(not(any(unix, windows)))]
        let _ = pids;
    });
}

/// Explicit native-app quit hook. Run off the UI thread and wait briefly for the
/// workers to reap their children, rather than leaving isolated provider groups.
///
/// Leaders get SIGTERM first so a CLI that is saving a rotated OAuth token
/// (Claude Code) can finish; only what is still running after that is killed.
pub fn shutdown_all() {
    QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
    let registered = || -> Vec<u32> {
        active()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect()
    };
    #[cfg(unix)]
    {
        for pid in registered() {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !registered().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    let pids = registered();
    for pid in pids {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let mut c = Command::new("taskkill");
            c.args(["/PID", &pid.to_string(), "/T", "/F"]);
            use std::os::windows::process::CommandExt;
            c.creation_flags(CREATE_NO_WINDOW);
            let _ = c.status();
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    fn spawn(script: &str) -> std::process::Child {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]).stdin(Stdio::piped());
        isolate(&mut command);
        command.spawn().unwrap()
    }
    #[test]
    // `retire_pid` reaps the child with waitpid.
    #[allow(clippy::zombie_processes)]
    fn retire_lets_a_child_finish_on_stdin_eof_before_any_signal() {
        let dir = std::env::temp_dir().join(format!("monocode-retire-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        // Exits 0 on EOF after a short write; a signal would skip the write.
        let mut child = spawn(&format!(
            "trap '' TERM; cat >/dev/null; sleep 0.2; echo saved > '{}'",
            dir.display()
        ));
        let pid = child.id();
        drop(child.stdin.take());
        let started = Instant::now();
        retire_pid(pid, || {});
        assert!(started.elapsed() < EXIT_GRACE);
        assert_eq!(std::fs::read_to_string(&dir).unwrap().trim(), "saved");
        let _ = std::fs::remove_file(&dir);
    }
    #[test]
    // `retire_pid` reaps the child with waitpid.
    #[allow(clippy::zombie_processes)]
    fn retire_escalates_when_a_child_ignores_stdin_and_sigterm() {
        // Keep one process: a shell loop leaves killed sleep grandchildren
        // whose reaping can keep the group visible briefly on Linux. SIG_IGN
        // survives exec, so this child still requires escalation to SIGKILL.
        let mut child = spawn("trap '' TERM; exec sleep 60");
        let pid = child.id();
        drop(child.stdin.take());
        let started = Instant::now();
        retire_pid(pid, || {});
        assert!(started.elapsed() >= EXIT_GRACE + TERM_GRACE - Duration::from_millis(100));
        // The group is sent SIGKILL as `retire_pid` returns; its other members
        // (the loop's `sleep`) can take a moment to exit and be reaped, longer
        // on a loaded CI runner.
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(-(pid as i32), 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(unsafe { libc::kill(-(pid as i32), 0) }, -1);
    }
    #[test]
    fn a_tracked_pid_is_listed_until_its_registration_drops() {
        // A pid no process has: only the registry is under test, nothing is signalled.
        const PID: u32 = 4_100_000;
        let listed = || agents().iter().any(|a| a.pid == PID);
        let registration = owned_by(77, || track(PID));
        assert!(listed());
        drop(registration);
        assert!(!listed());
    }
    #[test]
    fn reset_signal_mask_alone_clears_a_blocked_sigchld() {
        const CHILD: &str = "MONOCODE_RESET_MASK_REGRESSION_CHILD";
        unsafe {
            let mut mask = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut mask);
            if std::env::var_os(CHILD).is_some() {
                assert_eq!(libc::sigismember(&mask, libc::SIGCHLD), 0);
                return;
            }
            let mut blocked = std::mem::zeroed();
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGCHLD);
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, std::ptr::null_mut()),
                0
            );
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "process::tests::reset_signal_mask_alone_clears_a_blocked_sigchld",
                    "--nocapture",
                ])
                .env(CHILD, "1");
            reset_signal_mask(&mut command);
            let result = command.output();
            libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
            let output = result.unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
    #[test]
    fn provider_child_does_not_inherit_executor_signal_mask() {
        const CHILD: &str = "MONOCODE_SIGNAL_REGRESSION_CHILD";
        unsafe {
            let mut mask = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut mask);
            if std::env::var_os(CHILD).is_some() {
                assert_eq!(libc::sigismember(&mask, libc::SIGCHLD), 0);
                return;
            }
            let mut blocked = std::mem::zeroed();
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGCHLD);
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, std::ptr::null_mut()),
                0
            );
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "process::tests::provider_child_does_not_inherit_executor_signal_mask",
                    "--nocapture",
                ])
                .env(CHILD, "1");
            isolate(&mut command);
            let result = command.output();
            libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
            let output = result.unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn background_children_keep_piped_output_and_exit_status() {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", "(echo stdout)&(echo stderr 1>&2)&exit /b 7"]);
        configure_child(&mut command);

        let output = command.output().expect("background command should run");
        assert_eq!(output.status.code(), Some(7));
        assert!(String::from_utf8_lossy(&output.stdout).contains("stdout"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("stderr"));
    }
}
