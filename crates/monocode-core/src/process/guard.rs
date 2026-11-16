//! Orphan guard for long-lived provider children (sign-in CLIs).
//!
//! The app kills these children itself on every orderly path, but a crash, a
//! `pkill` or a Force Quit leaves them reparented to init with their listener
//! sockets open. This guard ties the child's whole process group to the app's
//! life by mechanisms the kernel runs even when the app is gone:
//!
//! * unix: a tiny `/bin/sh` watchdog joins the child's group and blocks reading
//!   a pipe whose write end only the app holds. When the app dies the pipe
//!   closes and the watchdog runs `kill -s KILL 0`, killing the whole group
//!   including itself. A process group cannot vanish (nor its id be reused)
//!   while a member lives, so the signal can never hit an unrelated process.
//! * unix safety net: a record file per child lets the next app start kill a
//!   group whose watchdog was killed too ([`sweep`]).
//! * windows: a job object with kill-on-close; the OS closes the handle when the
//!   app dies.
//!
//! Use [`arm`] only for a command whose argv carries no secret: the program and
//! arguments are written to the record file.
use std::{
    collections::HashMap,
    path::Path,
    process::{Child, Command},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

#[cfg(unix)]
use std::{
    path::PathBuf,
    process::{ChildStdin, Stdio},
    time::{Duration, Instant},
};

/// What `kill_all` needs to end one armed group.
#[derive(Clone, Copy)]
enum Target {
    #[cfg(unix)]
    Group(i32),
    #[cfg(windows)]
    Job(isize),
    #[cfg(not(any(unix, windows)))]
    Nothing,
}

fn registry() -> &'static Mutex<HashMap<u64, Target>> {
    static ARMED: OnceLock<Mutex<HashMap<u64, Target>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(HashMap::new()))
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Ties one child (and its descendants) to the life of this process.
/// Dropping it or calling [`ChildGuard::finish`] releases the guard.
pub struct ChildGuard {
    id: u64,
    #[cfg(unix)]
    watchdog: Option<(Child, ChildStdin)>,
    #[cfg(unix)]
    record: Option<PathBuf>,
    #[cfg(windows)]
    job: isize,
    armed: bool,
}

#[cfg(unix)]
const WATCHDOG_SCRIPT: &str = "read -r _ ; kill -s KILL 0";
#[cfg(unix)]
const STALE_TEMPORARY: Duration = Duration::from_secs(60);
#[cfg(unix)]
const RELEASE_WAIT: Duration = Duration::from_secs(2);

#[cfg(test)]
thread_local! {
    static LAST_WATCHDOG: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}
/// Pid of the watchdog the last `arm` on this thread started.
#[cfg(all(test, unix))]
pub(crate) fn last_watchdog_pid() -> Option<u32> {
    LAST_WATCHDOG.get()
}

#[cfg(windows)]
fn disarmed() -> ChildGuard {
    ChildGuard {
        id: 0,
        job: 0,
        armed: false,
    }
}

/// Call right after spawning a child that was spawned with `process::isolate`.
/// Never fails: on any error it logs a short warning and returns a guard that
/// does nothing. Do not use it for a command whose argv carries a secret.
pub fn arm(child: &Child, command: &Command) -> ChildGuard {
    arm_in(child, command, RECORDS.get().map(|dir| dir.as_path()))
}

/// [`arm`] with an explicit records directory (`None` writes no record).
fn arm_in(child: &Child, command: &Command, records: Option<&Path>) -> ChildGuard {
    #[cfg(unix)]
    {
        arm_unix(child, command, records)
    }
    #[cfg(windows)]
    {
        let _ = (command, records);
        arm_windows(child)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (child, command, records);
        ChildGuard {
            id: 0,
            armed: false,
        }
    }
}

#[cfg(unix)]
fn arm_unix(child: &Child, command: &Command, records: Option<&Path>) -> ChildGuard {
    use std::os::unix::process::CommandExt;
    let leader = child.id();
    let mut watchdog = Command::new("/bin/sh");
    watchdog
        .args(["-c", WATCHDOG_SCRIPT, "login-orphan-watchdog"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(leader as i32);
    super::reset_signal_mask(&mut watchdog);
    let watchdog = match watchdog.spawn() {
        Ok(mut dog) => match dog.stdin.take() {
            Some(pipe) => {
                #[cfg(test)]
                LAST_WATCHDOG.set(Some(dog.id()));
                Some((dog, pipe))
            }
            None => {
                let _ = dog.kill();
                let _ = dog.wait();
                None
            }
        },
        Err(error) => {
            eprintln!("orphan guard: watchdog did not start: {error}");
            None
        }
    };
    let record = records.and_then(|dir| write_record(dir, leader, command));
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, Target::Group(leader as i32));
    ChildGuard {
        id,
        watchdog,
        record,
        armed: true,
    }
}

#[cfg(windows)]
fn arm_windows(child: &Child) -> ChildGuard {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
        },
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            eprintln!("orphan guard: could not create a job object");
            return disarmed();
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        // Silent breakaway keeps only the CLI process itself in the job: a browser
        // the CLI launches is not a member, so closing the job after a successful
        // sign-in cannot kill it. The CLI still dies with the app; its descendants
        // are ended by `taskkill /T` on cancel and quit instead.
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
        let configured = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if configured == 0 || AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
            eprintln!("orphan guard: could not place the child in a job object");
            CloseHandle(job);
            return disarmed();
        }
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, Target::Job(job as isize));
        ChildGuard {
            id,
            job: job as isize,
            armed: true,
        }
    }
}

impl ChildGuard {
    /// Release the guard: stop the watchdog (it clears what is left of the
    /// group), reap it and forget the record. Bounded and idempotent.
    pub fn finish(mut self) {
        self.release();
    }

    /// Stop the child and release the guard in one call. The registry entry goes
    /// first, so `kill_all` can never signal a group id after the leader is reaped.
    pub fn finish_with(mut self, child: &mut Child) {
        self.unregister();
        super::terminate(child);
        self.release();
    }

    fn unregister(&self) {
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }

    #[cfg(all(test, unix))]
    pub(crate) fn watchdog_pid(&self) -> Option<u32> {
        self.watchdog.as_ref().map(|(dog, _)| dog.id())
    }

    /// `kill_all` restricted to this guard, so parallel tests keep their groups.
    #[cfg(all(test, unix))]
    pub(crate) fn kill_group(&self) {
        let id = self.id;
        kill_matching(|candidate| candidate == id);
    }

    fn release(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        // Under the registry lock `kill_matching` holds while signalling, so a
        // handle is never used after it is closed below.
        self.unregister();
        #[cfg(unix)]
        {
            if let Some((mut dog, pipe)) = self.watchdog.take() {
                // EOF on its stdin makes the watchdog kill the group and itself.
                drop(pipe);
                let deadline = Instant::now() + RELEASE_WAIT;
                let mut reaped = false;
                while Instant::now() < deadline {
                    match dog.try_wait() {
                        Ok(Some(_)) | Err(_) => {
                            reaped = true;
                            break;
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
                if !reaped {
                    let _ = dog.kill();
                    let _ = dog.wait();
                }
            }
            if let Some(path) = self.record.take() {
                let _ = std::fs::remove_file(path);
            }
        }
        #[cfg(windows)]
        unsafe {
            // Kill-on-close ends anything still in the job.
            windows_sys::Win32::Foundation::CloseHandle(self.job as _);
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// Kill every armed group now. Instant and non-blocking: safe in a quit hook.
pub fn kill_all() {
    kill_matching(|_| true);
}

fn kill_matching(select: impl Fn(u64) -> bool) {
    // Signal while holding the lock: `release` unregisters (and only then closes
    // its handle) under the same lock. Signalling is instant and never blocks.
    let armed = registry().lock().unwrap_or_else(|e| e.into_inner());
    for (_, target) in armed.iter().filter(|(id, _)| select(**id)) {
        match *target {
            #[cfg(unix)]
            Target::Group(pgid) => unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            },
            #[cfg(windows)]
            Target::Job(job) => unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job as _, 1);
            },
            #[cfg(not(any(unix, windows)))]
            Target::Nothing => {}
        }
    }
}

static RECORDS: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Remember `records_dir` for the record files and sweep it once on a
/// background thread. Without `init` no records are written.
pub fn init(records_dir: &Path) {
    let dir = records_dir.to_path_buf();
    if RECORDS.set(dir.clone()).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        let _ = std::thread::Builder::new()
            .name("orphan-guard-sweep".into())
            .spawn(move || sweep(&dir));
    }
}

#[cfg(unix)]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub(crate) struct Record {
    pub pid: u32,
    pub started: String,
    /// The program, then the arguments the app passed (never secrets).
    pub command: Vec<String>,
    pub app_pid: u32,
    pub app_started: String,
}

/// Start stamp of a process, stable for its lifetime and different after pid reuse.
#[cfg(target_os = "macos")]
pub(crate) fn start_stamp(pid: u32) -> Option<String> {
    crate::resources::identity(pid)
}
#[cfg(target_os = "linux")]
pub(crate) fn start_stamp(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may contain spaces and parentheses: fields start after the last ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    // `rest` begins at field 3 (state); starttime is field 22.
    rest.split_whitespace().nth(19).map(str::to_owned)
}
#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
pub(crate) fn start_stamp(_pid: u32) -> Option<String> {
    None
}

/// Command line of a process as the OS reports it (program first).
#[cfg(target_os = "linux")]
pub(crate) fn argv(pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    Some(
        text.strip_suffix('\0')?
            .split('\0')
            .map(str::to_owned)
            .collect(),
    )
}
#[cfg(target_os = "macos")]
pub(crate) fn argv(pid: u32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size = 0usize;
    let mut query = |buffer: *mut libc::c_void, size: &mut usize| unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, buffer, size, std::ptr::null_mut(), 0)
    };
    if query(std::ptr::null_mut(), &mut size) != 0 || size < 4 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    if query(buffer.as_mut_ptr() as *mut libc::c_void, &mut size) != 0 {
        return None;
    }
    buffer.truncate(size);
    // Layout: int argc, the executable path, NUL padding, then argc strings.
    let argc = i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?);
    let rest = &buffer[4..];
    let path_end = rest.iter().position(|byte| *byte == 0)?;
    let rest = &rest[path_end..];
    let start = rest.iter().position(|byte| *byte != 0)?;
    let mut parts = rest[start..].split(|byte| *byte == 0);
    let mut args = Vec::new();
    for _ in 0..usize::try_from(argc).ok()? {
        args.push(String::from_utf8(parts.next()?.to_vec()).ok()?);
    }
    Some(args)
}
#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
pub(crate) fn argv(_pid: u32) -> Option<Vec<String>> {
    None
}

#[cfg(unix)]
fn write_record(dir: &Path, leader: u32, command: &Command) -> Option<PathBuf> {
    let record = Record {
        pid: leader,
        started: start_stamp(leader)?,
        command: std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|part| part.to_string_lossy().into_owned())
            .collect(),
        app_pid: std::process::id(),
        // Without our own stamp a later sweep could not tell a live app from a dead one.
        app_started: start_stamp(std::process::id())?,
    };
    match save_record(dir, &record) {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!("orphan guard: could not write the record: {error}");
            None
        }
    }
}

#[cfg(unix)]
pub(crate) fn save_record(dir: &Path, record: &Record) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", record.pid));
    let temporary = dir.join(format!("{}.json.tmp", record.pid));
    std::fs::write(&temporary, serde_json::to_vec(record)?)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

/// Kill the login groups an app that died abruptly left behind. A group is
/// killed only when its leader is provably the recorded process: same start
/// stamp, matching arguments and still the group leader. Records of a live app
/// are skipped; every other record is deleted.
#[cfg(unix)]
pub fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // `file_type` does not follow symlinks: only regular files count.
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.ends_with(".json.tmp"))
        {
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age > STALE_TEMPORARY);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
            continue;
        }
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let path = entry.path();
        let record = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok());
        let Some(record) = record else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if app_is_alive(&record) {
            continue;
        }
        if is_recorded_leader(&record) {
            unsafe { libc::kill(-(record.pid as i32), libc::SIGKILL) };
        }
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(unix)]
fn app_is_alive(record: &Record) -> bool {
    let exists = unsafe { libc::kill(record.app_pid as i32, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && start_stamp(record.app_pid).is_some_and(|now| now == record.app_started)
}

/// The live command line must end with the recorded arguments, so a wrapper
/// (`node /x/cli.js auth login`) still matches `["auth", "login"]`. Without
/// recorded arguments or a readable command line nothing matches.
#[cfg(unix)]
fn argv_matches(record: &Record, pid: u32) -> bool {
    let wanted = record.command.get(1..).unwrap_or_default();
    !wanted.is_empty()
        && argv(pid).is_some_and(|live| live.len() >= wanted.len() && live.ends_with(wanted))
}

#[cfg(unix)]
fn is_recorded_leader(record: &Record) -> bool {
    if record.pid == 0 || record.pid > i32::MAX as u32 {
        return false;
    }
    let pid = record.pid;
    start_stamp(pid).is_some_and(|stamp| stamp == record.started)
        && argv_matches(record, pid)
        && unsafe { libc::getpgid(pid as i32) } == pid as i32
}

#[cfg(all(test, unix))]
pub(crate) mod testutil {
    use std::time::{Duration, Instant};

    /// A process counts as gone when it does not exist or is a zombie.
    pub fn gone(pid: i32) -> bool {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| {
                    stat.rfind(')')
                        .and_then(|at| stat[at + 1..].trim_start().chars().next())
                })
                .is_none_or(|state| state == 'Z')
        }
        #[cfg(not(target_os = "linux"))]
        {
            let state = std::process::Command::new("ps")
                .args(["-o", "state=", "-p", &pid.to_string()])
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
                .unwrap_or_default();
            state.is_empty() || state.starts_with('Z')
        }
    }
    pub fn wait_gone(pid: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if gone(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        gone(pid)
    }
    /// SIGKILLs the recorded pids when a test ends, even by a failed assertion.
    #[derive(Default)]
    pub struct KillOnDrop(pub Vec<i32>);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            for pid in &self.0 {
                if *pid > 1 && !gone(*pid) {
                    unsafe { libc::kill(*pid, libc::SIGKILL) };
                }
            }
        }
    }
    pub fn read_pid(path: &std::path::Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "no pid in {}", path.display());
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::testutil::*;
    use super::*;

    /// A group leader that starts a grandchild and records both pids.
    fn spawn_group(dir: &Path) -> (Command, Child) {
        let script = format!("sleep 300 & echo $! > '{0}/grand'; wait", dir.display());
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &script]);
        crate::process::isolate(&mut command);
        command.stdin(Stdio::null()).stdout(Stdio::null());
        let child = command.spawn().unwrap();
        (command, child)
    }

    #[test]
    fn kill_all_kills_an_armed_group_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (command, mut child) = spawn_group(dir.path());
        let leader = child.id() as i32;
        let guard = arm(&child, &command);
        let mut cleanup = KillOnDrop(vec![leader]);
        let grand = read_pid(&dir.path().join("grand"));
        cleanup.0.push(grand);
        // The `kill_all` path, limited to this guard so parallel tests are left alone.
        guard.kill_group();
        assert!(wait_gone(grand));
        assert!(wait_gone(leader));
        let _ = child.wait();
        guard.finish();
    }

    #[test]
    fn finish_reaps_the_watchdog_of_a_terminated_group() {
        let dir = tempfile::tempdir().unwrap();
        let (command, mut child) = spawn_group(dir.path());
        let guard = arm(&child, &command);
        let mut cleanup = KillOnDrop(vec![child.id() as i32]);
        let grand = read_pid(&dir.path().join("grand"));
        cleanup.0.push(grand);
        let watchdog = guard.watchdog_pid().expect("a watchdog") as i32;
        crate::process::terminate(&mut child);
        let started = Instant::now();
        guard.finish();
        assert!(started.elapsed() < Duration::from_secs(2));
        // Reaped, not left as a zombie: the pid no longer exists at all.
        assert_ne!(unsafe { libc::kill(watchdog, 0) }, 0);
        assert!(wait_gone(grand));
    }

    #[test]
    fn dropping_the_guard_kills_what_is_left_of_the_group() {
        let dir = tempfile::tempdir().unwrap();
        let (command, mut child) = spawn_group(dir.path());
        let guard = arm(&child, &command);
        let mut cleanup = KillOnDrop(vec![child.id() as i32]);
        let grand = read_pid(&dir.path().join("grand"));
        cleanup.0.push(grand);
        drop(guard);
        assert!(wait_gone(grand));
        let _ = child.wait();
    }

    /// A live isolated process plus a record describing it.
    struct Subject {
        child: Child,
        record: Record,
        dir: tempfile::TempDir,
    }
    impl Drop for Subject {
        fn drop(&mut self) {
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            let _ = self.child.wait();
        }
    }
    fn dead_pid() -> u32 {
        let mut dead = Command::new("/bin/sh").args(["-c", ":"]).spawn().unwrap();
        dead.wait().unwrap();
        dead.id()
    }
    fn subject() -> Subject {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 300; :"]);
        crate::process::isolate(&mut command);
        command.stdin(Stdio::null()).stdout(Stdio::null());
        let child = command.spawn().unwrap();
        let pid = child.id();
        let record = Record {
            pid,
            started: start_stamp(pid).expect("a start stamp"),
            command: vec!["/bin/sh".into(), "-c".into(), "sleep 300; :".into()],
            app_pid: dead_pid(),
            app_started: "0".into(),
        };
        Subject {
            child,
            record,
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn file(subject: &Subject) -> PathBuf {
        save_record(&subject.dir.path().join("g"), &subject.record).unwrap()
    }
    fn swept(subject: &Subject) {
        sweep(&subject.dir.path().join("g"));
    }

    #[test]
    fn sweep_never_kills_without_recorded_arguments() {
        let mut subject = subject();
        subject.record.command = vec!["/bin/sh".into()];
        let path = file(&subject);
        swept(&subject);
        assert!(!gone(subject.record.pid as i32));
        assert!(!path.exists());
    }

    #[test]
    fn sweep_matches_a_wrapper_by_the_end_of_its_command_line() {
        let live = argv(std::process::id()).expect("own command line");
        let record = |args: &[&str]| Record {
            pid: std::process::id(),
            started: String::new(),
            command: std::iter::once("wrapper".to_owned())
                .chain(args.iter().map(|arg| arg.to_string()))
                .collect(),
            app_pid: 0,
            app_started: String::new(),
        };
        let last = live.last().unwrap().as_str();
        assert!(argv_matches(&record(&[last]), std::process::id()));
        assert!(!argv_matches(
            &record(&["definitely-not-an-argument"]),
            std::process::id()
        ));
    }

    #[test]
    fn a_record_exists_while_armed_and_is_removed_by_finish() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let (mut command, mut child) = spawn_group(work.path());
        command.arg("auth");
        let guard = arm_in(&child, &command, Some(dir.path()));
        let mut cleanup = KillOnDrop(vec![child.id() as i32]);
        let path = dir.path().join(format!("{}.json", child.id()));
        let text = std::fs::read_to_string(&path).expect("a record");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["pid"], child.id());
        assert_eq!(value["command"][0], "/bin/sh");
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["app_pid", "app_started", "command", "pid", "started"]
        );
        assert!(!dir.path().join(format!("{}.json.tmp", child.id())).exists());
        cleanup.0.push(read_pid(&work.path().join("grand")));
        guard.finish_with(&mut child);
        assert!(!path.exists());
    }

    #[test]
    fn sweep_removes_old_temporary_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("1.json.tmp");
        let fresh = dir.path().join("2.json.tmp");
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&fresh, "x").unwrap();
        let file = std::fs::File::options().write(true).open(&old).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
            .unwrap();
        sweep(dir.path());
        assert!(!old.exists());
        assert!(fresh.exists());
    }

    #[test]
    fn sweep_kills_a_matching_group_of_a_dead_app() {
        let subject = subject();
        let path = file(&subject);
        swept(&subject);
        assert!(wait_gone(subject.record.pid as i32));
        assert!(!path.exists());
    }

    #[test]
    fn sweep_spares_a_process_with_other_arguments() {
        let mut subject = subject();
        subject.record.command = vec!["/bin/sh".into(), "auth".into(), "login".into()];
        let path = file(&subject);
        swept(&subject);
        assert!(!gone(subject.record.pid as i32));
        assert!(!path.exists());
    }

    #[test]
    fn sweep_spares_a_process_with_another_start_stamp() {
        let mut subject = subject();
        subject.record.started.push('0');
        let path = file(&subject);
        swept(&subject);
        assert!(!gone(subject.record.pid as i32));
        assert!(!path.exists());
    }

    #[test]
    fn sweep_skips_the_records_of_a_live_app() {
        let mut subject = subject();
        subject.record.app_pid = std::process::id();
        subject.record.app_started = start_stamp(std::process::id()).unwrap();
        let path = file(&subject);
        swept(&subject);
        assert!(!gone(subject.record.pid as i32));
        assert!(path.exists());
    }

    #[test]
    fn sweep_drops_the_record_of_a_missing_process() {
        let mut subject = subject();
        let real = subject.record.pid;
        subject.record.pid = dead_pid();
        let path = file(&subject);
        swept(&subject);
        assert!(!path.exists());
        assert!(!gone(real as i32));
    }

    #[test]
    fn sweep_drops_unreadable_records_and_ignores_other_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("123.json"), "not json").unwrap();
        std::fs::write(dir.path().join("notes.json"), "keep").unwrap();
        sweep(dir.path());
        assert!(!dir.path().join("123.json").exists());
        assert!(dir.path().join("notes.json").exists());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn closing_the_guard_kills_the_job() {
        // The job holds only this process (silent breakaway), so use `ping`
        // itself: a failure then cannot leak a descendant.
        let mut command = Command::new("ping");
        command.args(["-n", "30", "127.0.0.1"]);
        crate::process::isolate(&mut command);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null());
        let mut child = command.spawn().unwrap();
        let guard = arm(&child, &command);
        assert!(child.try_wait().unwrap().is_none());
        drop(guard);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let ended = loop {
            if child.try_wait().unwrap().is_some() {
                break true;
            }
            if std::time::Instant::now() > deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        if !ended {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(ended, "the job must end its child when the guard closes");
    }
}
