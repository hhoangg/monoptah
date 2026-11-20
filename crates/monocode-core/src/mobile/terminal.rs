//! The web terminal: a shell on this desktop for a web device, over the
//! relay's end-to-end channel. Everything that decides is here, in the
//! desktop, and nothing the web server sends can switch any of it on.
//!
//! - **Who may open one.** [`super::policy`] gates the methods: a web device
//!   with Control and the desktop's own "Allow web terminal" toggle for that
//!   device (default off). A read-only device never gets it.
//! - **Re-auth on every open.** The desktop makes a short one-time code and
//!   shows it only on its own screen ([`Hub::status`]). The browser must send
//!   that code back: a code is bound to one device and project, lives
//!   [`Limits::code_ttl_ms`], allows [`Limits::max_attempts`] tries, works once
//!   and is compared in constant time. Repeated failures lock the device out.
//! - **Limits the desktop enforces.** Idle (no input) and maximum duration,
//!   and a grace window after the link drops: a shell nobody reattaches within
//!   [`Limits::grace_ms`] is killed.
//! - **Kill, never hang.** [`Hub::kill_device`], [`Hub::kill_all`] and the tick
//!   end a shell and its whole process group through [`ShellHandle::kill`],
//!   which must not block. Revocation, a lowered permission, the toggle going
//!   off and the user's Kill button all end up here, and [`Hub::tick`]
//!   re-checks every device's standing as a safety net.
//! - **Flow control.** The shell's output goes to a bounded [`Output`] window.
//!   The browser acknowledges what it has by asking for the next offset; until
//!   it does, the producer stops reading the pseudo-terminal, so the shell
//!   itself slows down. Nothing is dropped and nothing grows without bound.
//!
//! The pseudo-terminal itself comes from the app through [`ShellSpawner`], so
//! this module and its tests need no operating system terminal. The clock is
//! injected for the same reason.
use super::audit::{self, Lifecycle, Transport};
use super::policy::Code;
use monocode_mobile_link::Failure;
use rand::Rng;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// The characters of a code: no `0`, `O`, `1`, `I` or `L`, 31 in all.
pub const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
/// Characters in a code.
pub const CODE_LEN: usize = 6;
/// Most bytes one `terminal.write` carries.
pub const MAX_INPUT_BYTES: usize = 8 * 1024;
/// Most bytes one `terminal.read` returns.
pub const MAX_READ_BYTES: usize = 32 * 1024;
/// Longest a `terminal.read` waits for output.
pub const MAX_WAIT_MS: u64 = 20_000;
/// Bounds of the terminal size a browser may ask for.
pub const COLS: std::ops::RangeInclusive<u16> = 10..=500;
pub const ROWS: std::ops::RangeInclusive<u16> = 2..=200;

/// Every duration and bound, so tests need not wait for real time.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// A code lives this long.
    pub code_ttl_ms: u64,
    /// Wrong guesses one code allows.
    pub max_attempts: u8,
    /// Failed codes of one device within `lockout_window_ms` that lock it out.
    pub lockout_failures: u32,
    pub lockout_window_ms: u64,
    /// How long a locked-out device gets no new code.
    pub lockout_ms: u64,
    /// Least time between two codes asked for by one device.
    pub code_gap_ms: u64,
    /// No input for this long ends the shell.
    pub idle_ms: u64,
    /// A shell never lives longer than this.
    pub max_ms: u64,
    /// A shell whose browser is gone is killed after this.
    pub grace_ms: u64,
    /// How long an ended shell stays readable, so its last output and the
    /// reason reach the browser.
    pub tombstone_ms: u64,
    pub per_device: usize,
    pub total: usize,
    /// Bytes the shell may write ahead of the browser's acknowledgement.
    pub window: usize,
    /// Codes shown at once, across devices.
    pub max_challenges: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            code_ttl_ms: 60_000,
            max_attempts: 3,
            lockout_failures: 5,
            lockout_window_ms: 10 * 60_000,
            lockout_ms: 10 * 60_000,
            code_gap_ms: 2_000,
            idle_ms: 15 * 60_000,
            max_ms: 8 * 60 * 60_000,
            grace_ms: 30_000,
            tombstone_ms: 60_000,
            per_device: 2,
            total: 4,
            window: 256 * 1024,
            max_challenges: 4,
        }
    }
}

/// Milliseconds on a clock that only moves forward.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The real clock.
pub struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Why a shell ended. The text reaches the browser as `exited.reason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The shell exited on its own.
    Exit,
    /// The browser asked to close it.
    Closed,
    /// The device was revoked, left, or lost Control.
    Revoked,
    Idle,
    Max,
    /// "Allow web terminal" was turned off.
    ToggleOff,
    /// The user pressed Kill on the desktop.
    Killed,
    /// The browser did not come back within the grace window.
    Grace,
    /// The desktop is shutting down or the link was switched off.
    Desktop,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Exit => "exit",
            Reason::Closed => "closed",
            Reason::Revoked => "revoked",
            Reason::Idle => "idle",
            Reason::Max => "max",
            Reason::ToggleOff => "toggle_off",
            Reason::Killed => "killed",
            Reason::Grace => "grace",
            Reason::Desktop => "desktop",
        }
    }
}

/// The device behind a request, as the host knows it from its own rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    /// The row id on this desktop.
    pub device_id: String,
    pub fingerprint: String,
    pub name: String,
    /// The relay's id for the device, to tell which link went away.
    pub relay_id: Option<String>,
}

/// Whether a device may still hold a shell: the host answers from its rows
/// and the policy toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Allowed,
    /// Gone, not a web device, or no longer Control.
    Revoked,
    ToggleOff,
}

pub type Rights = Arc<dyn Fn(&str) -> Standing + Send + Sync>;

/// A running shell, as the hub sees it.
pub trait ShellHandle: Send + Sync {
    /// Writes keystrokes. May wait briefly, never for long.
    fn write(&self, data: &[u8]) -> Result<(), String>;
    fn resize(&self, cols: u16, rows: u16);
    /// Kills the shell and its whole process group. Must not block, and may
    /// be called more than once.
    fn kill(&self);
}

/// What to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

/// Starts shells on a pseudo-terminal. The app provides it. The backend pumps
/// the shell's output into `out` with [`Output::push`] (which blocks while the
/// browser is behind) and calls [`Output::finish`] when the shell ends.
pub trait ShellSpawner: Send + Sync {
    fn spawn(&self, spec: SpawnSpec, out: Arc<Output>) -> Result<Arc<dyn ShellHandle>, String>;
}

struct OutState {
    /// Offset of the first byte in `buf`.
    base: u64,
    buf: VecDeque<u8>,
    closed: bool,
    exit: Option<Option<i32>>,
}

/// The shell's output, between the backend (producer) and the hub.
pub struct Output {
    state: Mutex<OutState>,
    space: Condvar,
    data: Notify,
    window: usize,
}

/// What [`Output::take`] hands out.
#[derive(Debug, PartialEq, Eq)]
pub struct Taken {
    pub from: u64,
    pub next: u64,
    pub data: Vec<u8>,
    /// Nothing is left in the window after `data`.
    pub drained: bool,
    /// The shell ended by itself, with this status.
    pub exit: Option<Option<i32>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Output {
    pub fn new(window: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(OutState {
                base: 0,
                buf: VecDeque::new(),
                closed: false,
                exit: None,
            }),
            space: Condvar::new(),
            data: Notify::new(),
            window,
        })
    }

    /// Adds output. Blocks while a full window is waiting for the browser to
    /// acknowledge it, so the shell is slowed instead of the buffer growing.
    /// Returns false once the shell was ended: stop reading.
    pub fn push(&self, data: &[u8]) -> bool {
        let mut state = lock(&self.state);
        while state.buf.len() >= self.window && !state.closed {
            state = self
                .space
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if state.closed {
            return false;
        }
        state.buf.extend(data);
        drop(state);
        self.data.notify_one();
        true
    }

    /// The shell ended by itself.
    pub fn finish(&self, code: Option<i32>) {
        lock(&self.state).exit = Some(code);
        self.data.notify_one();
    }

    /// Ends the output: a blocked producer returns, and later pushes fail.
    pub fn close(&self) {
        lock(&self.state).closed = true;
        self.space.notify_all();
        self.data.notify_one();
    }

    pub fn is_closed(&self) -> bool {
        lock(&self.state).closed
    }

    fn exit(&self) -> Option<Option<i32>> {
        lock(&self.state).exit
    }

    /// Acknowledges everything before `from` (dropping it) and returns up to
    /// `max` bytes from there. A `from` outside the window is clamped into it.
    pub fn take(&self, from: u64, max: usize) -> Taken {
        let mut state = lock(&self.state);
        let end = state.base + state.buf.len() as u64;
        let from = from.clamp(state.base, end);
        let drop_n = usize::try_from(from - state.base).unwrap_or(usize::MAX);
        let drop_n = drop_n.min(state.buf.len());
        state.buf.drain(..drop_n);
        state.base = from;
        let n = state.buf.len().min(max);
        let data: Vec<u8> = state.buf.iter().take(n).copied().collect();
        let drained = state.buf.len() == n;
        let taken = Taken {
            from,
            next: from + n as u64,
            data,
            drained,
            exit: state.exit,
        };
        drop(state);
        self.space.notify_all();
        taken
    }

    /// Bytes waiting in the window.
    pub fn buffered(&self) -> usize {
        lock(&self.state).buf.len()
    }
}

enum Phase {
    Live,
    /// The browser's link is gone; since when.
    Detached(u64),
    Ended {
        reason: Reason,
        at: u64,
        code: Option<i32>,
    },
}

struct Term {
    owner: Owner,
    project: String,
    label: String,
    opened_at: u64,
    last_input: u64,
    phase: Phase,
    handle: Arc<dyn ShellHandle>,
    out: Arc<Output>,
    /// Wall-clock open time for the desktop's indicator (Unix seconds).
    opened_unix: u64,
}

struct Challenge {
    id: String,
    owner: Owner,
    project: String,
    label: String,
    code: String,
    expires: u64,
    attempts_left: u8,
}

#[derive(Default)]
struct Failures {
    /// Failure times inside the window.
    at: Vec<u64>,
    locked_until: u64,
}

#[derive(Default)]
struct State {
    challenges: Vec<Challenge>,
    terminals: HashMap<String, Term>,
    failures: HashMap<String, Failures>,
    last_code: HashMap<String, u64>,
    /// Device ids of shells being started: they count against the limits
    /// before they are in `terminals`, so two opens cannot both slip in.
    starting: Vec<String>,
    ticking: bool,
}

/// Work to do once the state lock is released: killing and auditing.
#[derive(Default)]
struct After {
    kill: Vec<Arc<dyn ShellHandle>>,
    audit: Vec<(Lifecycle, Owner, Option<String>)>,
}

impl After {
    fn run(self) {
        for handle in self.kill {
            handle.kill();
        }
        for (kind, owner, detail) in self.audit {
            audit::record_lifecycle(
                kind,
                &owner.device_id,
                &owner.fingerprint,
                Transport::Web,
                detail.as_deref(),
            );
        }
    }
}

/// An open shell, for the desktop's indicator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTerminal {
    pub id: String,
    pub device_id: String,
    pub device_name: String,
    pub project: String,
    pub opened_unix: u64,
    /// The browser is not connected right now.
    pub detached: bool,
}

/// A code waiting for the user to read it to the browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCode {
    pub device_name: String,
    pub project: String,
    pub code: String,
    pub expires_in_ms: u64,
}

/// What the desktop shows: shells that are open and codes to read out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalStatus {
    pub open: Vec<OpenTerminal>,
    pub codes: Vec<PendingCode>,
}

/// The answer to the first `terminal.open` step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChallengeInfo {
    pub challenge_id: String,
    pub expires_in_ms: u64,
    pub attempts: u8,
}

impl ChallengeInfo {
    pub fn to_value(&self) -> Value {
        json!({
            "challengeId": self.challenge_id,
            "expiresInMs": self.expires_in_ms,
            "codeLength": CODE_LEN,
            "attempts": self.attempts,
        })
    }
}

/// What the second `terminal.open` step asks for, once the host resolved the
/// project to a folder.
pub struct OpenRequest {
    pub project: String,
    pub label: String,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub challenge_id: String,
    pub code: String,
}

/// An opened shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub id: String,
    pub cols: u16,
    pub rows: u16,
    pub label: String,
    pub idle_ms: u64,
    pub max_ms: u64,
}

impl Opened {
    pub fn to_value(&self) -> Value {
        json!({
            "terminalId": self.id,
            "cols": self.cols,
            "rows": self.rows,
            "cwd": self.label,
            "idleTimeoutMs": self.idle_ms,
            "maxDurationMs": self.max_ms,
        })
    }
}

/// An answer of `terminal.read`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadOut {
    pub from: u64,
    pub next: u64,
    pub data: Vec<u8>,
    pub exited: Option<(Reason, Option<i32>)>,
}

impl ReadOut {
    pub fn to_value(&self) -> Value {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let exited = self.exited.map(|(reason, code)| {
            let mut value = json!({ "reason": reason.as_str() });
            if let Some(code) = code {
                value["code"] = json!(code);
            }
            value
        });
        json!({
            "from": self.from,
            "next": self.next,
            "data": STANDARD.encode(&self.data),
            "exited": exited,
        })
    }
}

fn random_hex(bytes: usize) -> String {
    let mut rng = rand::rng();
    (0..bytes)
        .map(|_| format!("{:02x}", rng.random::<u8>()))
        .collect()
}

fn random_code() -> String {
    let mut rng = rand::rng();
    (0..CODE_LEN)
        .map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// Constant-time equality of two byte strings. The lengths are not secret.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0u8;
    for (x, y) in a.iter().zip(b) {
        difference |= x ^ y;
    }
    std::hint::black_box(difference) == 0
}

/// A typed code as stored: upper case, without spaces or hyphens.
fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn failure(code: Code) -> Failure {
    code.failure()
}

fn busy(text: &str) -> Failure {
    Failure::coded(Code::Busy.as_str(), text)
}

/// Releases the slot `open` reserved unless the shell got registered.
struct SlotGuard<'a> {
    hub: &'a Hub,
    device_id: &'a str,
    armed: bool,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.hub.release_slot(self.device_id);
        }
    }
}

/// The shells of one desktop and the codes that open them.
pub struct Hub {
    clock: Arc<dyn Clock>,
    limits: Limits,
    state: Mutex<State>,
    spawner: Mutex<Option<Arc<dyn ShellSpawner>>>,
    rights: Mutex<Option<Rights>>,
}

impl Hub {
    pub fn new(clock: Arc<dyn Clock>, limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            clock,
            limits,
            state: Mutex::default(),
            spawner: Mutex::default(),
            rights: Mutex::default(),
        })
    }

    pub fn with_defaults() -> Arc<Self> {
        Self::new(Arc::new(SystemClock::default()), Limits::default())
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The app's shell backend. Without one, opening answers `unsupported`.
    pub fn set_spawner(&self, spawner: Arc<dyn ShellSpawner>) {
        *lock(&self.spawner) = Some(spawner);
    }

    /// Where the hub asks whether a device still has the right to a shell.
    pub fn set_rights(&self, rights: Rights) {
        *lock(&self.rights) = Some(rights);
    }

    /// Step one of opening: shows a code on the desktop. No shell yet.
    pub fn challenge(
        &self,
        owner: &Owner,
        project: &str,
        label: &str,
    ) -> Result<ChallengeInfo, Failure> {
        if lock(&self.spawner).is_none() {
            return Err(failure(Code::Unsupported));
        }
        let now = self.clock.now_ms();
        let mut after = After::default();
        let result = {
            let mut state = lock(&self.state);
            prune(&mut state, now, &self.limits);
            let locked = state
                .failures
                .get(&owner.device_id)
                .is_some_and(|f| f.locked_until > now);
            let mine = state
                .terminals
                .values()
                .filter(|t| {
                    t.owner.device_id == owner.device_id && !matches!(t.phase, Phase::Ended { .. })
                })
                .count();
            let all = state
                .terminals
                .values()
                .filter(|t| !matches!(t.phase, Phase::Ended { .. }))
                .count();
            let recent = state
                .last_code
                .get(&owner.device_id)
                .is_some_and(|at| now.saturating_sub(*at) < self.limits.code_gap_ms);
            if locked {
                after.audit.push((
                    Lifecycle::TerminalReauthFailed,
                    owner.clone(),
                    Some("locked".into()),
                ));
                Err(busy("Too many wrong codes. Try again later."))
            } else if mine >= self.limits.per_device || all >= self.limits.total {
                Err(busy("Too many terminals are open"))
            } else if recent {
                Err(busy("Wait a moment before asking for another code"))
            } else {
                // One code per device: a new one replaces the old.
                state
                    .challenges
                    .retain(|c| c.owner.device_id != owner.device_id);
                if state.challenges.len() >= self.limits.max_challenges {
                    Err(busy("Too many codes are waiting on the desktop"))
                } else {
                    let id = random_hex(16);
                    state.last_code.insert(owner.device_id.clone(), now);
                    state.challenges.push(Challenge {
                        id: id.clone(),
                        owner: owner.clone(),
                        project: project.to_owned(),
                        label: label.to_owned(),
                        code: random_code(),
                        expires: now + self.limits.code_ttl_ms,
                        attempts_left: self.limits.max_attempts,
                    });
                    after
                        .audit
                        .push((Lifecycle::TerminalChallenge, owner.clone(), None));
                    Ok(ChallengeInfo {
                        challenge_id: id,
                        expires_in_ms: self.limits.code_ttl_ms,
                        attempts: self.limits.max_attempts,
                    })
                }
            }
        };
        after.run();
        result
    }

    /// Step two: checks the code and starts the shell.
    pub fn open(&self, owner: &Owner, request: OpenRequest) -> Result<Opened, Failure> {
        let spawner = lock(&self.spawner)
            .clone()
            .ok_or_else(|| failure(Code::Unsupported))?;
        let now = self.clock.now_ms();
        let mut after = After::default();
        let verified = {
            let mut state = lock(&self.state);
            prune(&mut state, now, &self.limits);
            let verified = self.verify(&mut state, owner, &request, now, &mut after);
            if verified.is_ok() {
                let mine = state
                    .terminals
                    .values()
                    .filter(|t| {
                        t.owner.device_id == owner.device_id
                            && !matches!(t.phase, Phase::Ended { .. })
                    })
                    .count()
                    + state
                        .starting
                        .iter()
                        .filter(|id| **id == owner.device_id)
                        .count();
                let all = state
                    .terminals
                    .values()
                    .filter(|t| !matches!(t.phase, Phase::Ended { .. }))
                    .count()
                    + state.starting.len();
                if mine >= self.limits.per_device || all >= self.limits.total {
                    Err(busy("Too many terminals are open"))
                } else {
                    // Reserved under the same lock as the check.
                    state.starting.push(owner.device_id.clone());
                    Ok(())
                }
            } else {
                verified
            }
        };
        after.run();
        verified?;
        // Gives the reserved slot back on every way out until it is registered,
        // including a spawner that panics.
        let mut slot = SlotGuard {
            hub: self,
            device_id: &owner.device_id,
            armed: true,
        };
        let out = Output::new(self.limits.window);
        let spawned = spawner.spawn(
            SpawnSpec {
                cwd: request.cwd.clone(),
                cols: request.cols,
                rows: request.rows,
            },
            out.clone(),
        );
        let handle = spawned.map_err(Failure::from)?;
        let id = random_hex(16);
        let mut after = After::default();
        // The device may have been revoked while the shell was starting. Asked
        // with no lock held: the host's answer takes its own lock.
        let standing = self.standing(&owner.device_id);
        let rejected = {
            let mut state = lock(&self.state);
            if let Some(at) = state.starting.iter().position(|d| *d == owner.device_id) {
                state.starting.swap_remove(at);
            }
            slot.armed = false;
            if standing.is_some_and(|s| s != Standing::Allowed) {
                true
            } else {
                state.terminals.insert(
                    id.clone(),
                    Term {
                        owner: owner.clone(),
                        project: request.project.clone(),
                        label: request.label.clone(),
                        opened_at: now,
                        last_input: now,
                        phase: Phase::Live,
                        handle: handle.clone(),
                        out: out.clone(),
                        opened_unix: unix_now(),
                    },
                );
                after
                    .audit
                    .push((Lifecycle::TerminalOpen, owner.clone(), None));
                false
            }
        };
        if rejected {
            out.close();
            handle.kill();
            return Err(failure(Code::Revoked));
        }
        after.run();
        Ok(Opened {
            id,
            cols: request.cols,
            rows: request.rows,
            label: request.label,
            idle_ms: self.limits.idle_ms,
            max_ms: self.limits.max_ms,
        })
    }

    /// Gives back a slot reserved by `open` whose shell did not start.
    fn release_slot(&self, device_id: &str) {
        let mut state = lock(&self.state);
        if let Some(at) = state.starting.iter().position(|d| d == device_id) {
            state.starting.swap_remove(at);
        }
    }

    /// Checks the code of `request`, with the attempt and lockout rules.
    fn verify(
        &self,
        state: &mut State,
        owner: &Owner,
        request: &OpenRequest,
        now: u64,
        after: &mut After,
    ) -> Result<(), Failure> {
        let required = || failure(Code::ReauthRequired);
        if state
            .failures
            .get(&owner.device_id)
            .is_some_and(|f| f.locked_until > now)
        {
            after.audit.push((
                Lifecycle::TerminalReauthFailed,
                owner.clone(),
                Some("locked".into()),
            ));
            return Err(busy("Too many wrong codes. Try again later."));
        }
        let Some(position) = state
            .challenges
            .iter()
            .position(|c| c.id == request.challenge_id)
        else {
            return Err(required());
        };
        let bound = {
            let c = &state.challenges[position];
            c.owner.device_id == owner.device_id
                && c.owner.fingerprint == owner.fingerprint
                && c.project == request.project
        };
        if !bound || state.challenges[position].expires <= now {
            state.challenges.remove(position);
            return Err(required());
        }
        let typed = normalize(&request.code);
        let right = {
            let c = &state.challenges[position];
            // Both sides are compared whatever the typed length, in constant
            // time for equal lengths.
            constant_time_eq(typed.as_bytes(), c.code.as_bytes())
        };
        if right {
            // Single use: the code is gone whether or not a shell follows.
            state.challenges.remove(position);
            return Ok(());
        }
        let exhausted = {
            let c = &mut state.challenges[position];
            c.attempts_left = c.attempts_left.saturating_sub(1);
            c.attempts_left == 0
        };
        if exhausted {
            state.challenges.remove(position);
        }
        let failures = state.failures.entry(owner.device_id.clone()).or_default();
        failures
            .at
            .retain(|at| now.saturating_sub(*at) < self.limits.lockout_window_ms);
        failures.at.push(now);
        let locked = failures.at.len() as u32 >= self.limits.lockout_failures;
        if locked {
            failures.locked_until = now + self.limits.lockout_ms;
            failures.at.clear();
            state
                .challenges
                .retain(|c| c.owner.device_id != owner.device_id);
        }
        after.audit.push((
            Lifecycle::TerminalReauthFailed,
            owner.clone(),
            Some(if locked { "locked" } else { "wrong_code" }.into()),
        ));
        Err(if exhausted || locked {
            required()
        } else {
            failure(Code::ReauthFailed)
        })
    }

    /// Long poll of the output from offset `from` (which acknowledges what
    /// came before). Returns as soon as there is output, the shell ended or
    /// `wait_ms` passed.
    pub async fn read(
        &self,
        owner: &Owner,
        id: &str,
        from: u64,
        wait_ms: u64,
        max: usize,
    ) -> Result<ReadOut, Failure> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(wait_ms.min(MAX_WAIT_MS));
        let max = max.clamp(1, MAX_READ_BYTES);
        loop {
            let (out, answer) = {
                let mut after = After::default();
                let mut state = lock(&self.state);
                let now = self.clock.now_ms();
                let term = owned(&mut state, owner, id, now, &self.limits)?;
                if matches!(term.phase, Phase::Detached(_)) {
                    term.phase = Phase::Live;
                }
                let out = term.out.clone();
                let taken = out.take(from, max);
                let answer = match term.phase {
                    Phase::Ended { reason, code, .. } => Some(ReadOut {
                        from: taken.from,
                        next: taken.next,
                        exited: taken.drained.then_some((reason, code)),
                        data: taken.data,
                    }),
                    _ if !taken.data.is_empty() => Some(ReadOut {
                        from: taken.from,
                        next: taken.next,
                        data: taken.data,
                        exited: None,
                    }),
                    _ => match taken.exit {
                        Some(code) => {
                            end(term, Reason::Exit, code, now, &mut after);
                            Some(ReadOut {
                                from: taken.from,
                                next: taken.next,
                                data: Vec::new(),
                                exited: Some((Reason::Exit, code)),
                            })
                        }
                        None => None,
                    },
                };
                drop(state);
                after.run();
                (out, answer)
            };
            if let Some(answer) = answer {
                return Ok(answer);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                let idle = out.take(from, 0);
                return Ok(ReadOut {
                    from: idle.from,
                    next: idle.next,
                    data: Vec::new(),
                    exited: None,
                });
            }
            tokio::select! {
                _ = out.data.notified() => {}
                _ = tokio::time::sleep(remaining) => {}
            }
        }
    }

    /// Keystrokes for a shell.
    pub fn write(&self, owner: &Owner, id: &str, data: &[u8]) -> Result<(), Failure> {
        if data.len() > MAX_INPUT_BYTES {
            return Err(Failure::from("Too much input at once"));
        }
        let handle = {
            let mut state = lock(&self.state);
            let now = self.clock.now_ms();
            let term = owned(&mut state, owner, id, now, &self.limits)?;
            live(term, now)?;
            term.handle.clone()
        };
        handle.write(data).map_err(Failure::from)
    }

    pub fn resize(&self, owner: &Owner, id: &str, cols: u16, rows: u16) -> Result<(), Failure> {
        let handle = {
            let mut state = lock(&self.state);
            let now = self.clock.now_ms();
            let term = owned(&mut state, owner, id, now, &self.limits)?;
            live(term, now)?;
            term.handle.clone()
        };
        handle.resize(cols, rows);
        Ok(())
    }

    /// The browser closes its shell.
    pub fn close(&self, owner: &Owner, id: &str) -> Result<(), Failure> {
        let mut after = After::default();
        {
            let mut state = lock(&self.state);
            let now = self.clock.now_ms();
            let term = owned(&mut state, owner, id, now, &self.limits)?;
            end(term, Reason::Closed, None, now, &mut after);
        }
        after.run();
        Ok(())
    }

    /// A relay link of a device ended (BYE, a CTRL "left", a lost socket).
    /// Its shells are detached: they live on for the grace window, and the
    /// same device reattaches by reading again.
    pub fn link_closed(&self, relay_id: &str) {
        let now = self.clock.now_ms();
        let mut state = lock(&self.state);
        for term in state.terminals.values_mut() {
            if matches!(term.phase, Phase::Live)
                && term
                    .owner
                    .relay_id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(relay_id))
            {
                term.phase = Phase::Detached(now);
            }
        }
    }

    /// Ends every shell of a device row and drops its pending codes.
    pub fn kill_device(&self, device_id: &str, reason: Reason) -> usize {
        self.kill_where(
            reason,
            |term| term.owner.device_id == device_id,
            |owner| owner.device_id == device_id,
        )
    }

    /// Ends every shell of a device key (used by the relay's revoke).
    pub fn kill_fingerprint(&self, fingerprint: &str, reason: Reason) -> usize {
        self.kill_where(
            reason,
            |term| term.owner.fingerprint == fingerprint,
            |owner| owner.fingerprint == fingerprint,
        )
    }

    /// Ends every shell and drops every code.
    pub fn kill_all(&self, reason: Reason) -> usize {
        self.kill_where(reason, |_| true, |_| true)
    }

    /// The desktop's Kill button.
    pub fn kill(&self, id: &str) -> bool {
        let mut after = After::default();
        let found = {
            let mut state = lock(&self.state);
            let now = self.clock.now_ms();
            match state.terminals.get_mut(id) {
                Some(term) if !matches!(term.phase, Phase::Ended { .. }) => {
                    end(term, Reason::Killed, None, now, &mut after);
                    true
                }
                _ => false,
            }
        };
        after.run();
        found
    }

    fn kill_where(
        &self,
        reason: Reason,
        shells: impl Fn(&Term) -> bool,
        codes: impl Fn(&Owner) -> bool,
    ) -> usize {
        let mut after = After::default();
        let mut count = 0;
        {
            let mut state = lock(&self.state);
            let now = self.clock.now_ms();
            state.challenges.retain(|c| !codes(&c.owner));
            for term in state.terminals.values_mut() {
                if !matches!(term.phase, Phase::Ended { .. }) && shells(term) {
                    end(term, reason, None, now, &mut after);
                    count += 1;
                }
            }
        }
        after.run();
        count
    }

    fn standing(&self, device_id: &str) -> Option<Standing> {
        let rights = lock(&self.rights).clone()?;
        Some(rights(device_id))
    }

    /// Ends the shells of a device whose project is no longer in `selected`.
    pub fn kill_unselected(&self, device_id: &str, selected: &[String]) -> usize {
        let mut after = After::default();
        let mut count = 0;
        {
            let mut state = lock(&self.state);
            let now = self.clock.now_ms();
            for term in state.terminals.values_mut() {
                if term.owner.device_id == device_id
                    && !matches!(term.phase, Phase::Ended { .. })
                    && !selected.contains(&term.project)
                {
                    end(term, Reason::Revoked, None, now, &mut after);
                    count += 1;
                }
            }
        }
        after.run();
        count
    }

    /// Enforces the limits and the devices' standing. Called every second by
    /// the ticker, and directly by tests with a manual clock.
    pub fn tick(&self) {
        // The standing of each device is asked with no lock held.
        let devices: Vec<String> = {
            let state = lock(&self.state);
            let mut ids: Vec<String> = state
                .terminals
                .values()
                .filter(|t| !matches!(t.phase, Phase::Ended { .. }))
                .map(|t| t.owner.device_id.clone())
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };
        let standing: HashMap<String, Standing> = devices
            .into_iter()
            .filter_map(|id| self.standing(&id).map(|s| (id, s)))
            .collect();
        let now = self.clock.now_ms();
        let mut after = After::default();
        {
            let mut state = lock(&self.state);
            prune(&mut state, now, &self.limits);
            let limits = self.limits;
            for term in state.terminals.values_mut() {
                let reason = match term.phase {
                    Phase::Ended { .. } => continue,
                    _ if standing.get(&term.owner.device_id) == Some(&Standing::Revoked) => {
                        Some(Reason::Revoked)
                    }
                    _ if standing.get(&term.owner.device_id) == Some(&Standing::ToggleOff) => {
                        Some(Reason::ToggleOff)
                    }
                    _ if now.saturating_sub(term.opened_at) >= limits.max_ms => Some(Reason::Max),
                    _ if now.saturating_sub(term.last_input) >= limits.idle_ms => {
                        Some(Reason::Idle)
                    }
                    Phase::Detached(since) if now.saturating_sub(since) >= limits.grace_ms => {
                        Some(Reason::Grace)
                    }
                    _ => None,
                };
                if let Some(reason) = reason {
                    end(term, reason, None, now, &mut after);
                } else if let Some(code) = term.out.exit() {
                    // The shell ended by itself: reap its group, keep its
                    // output readable until the browser has taken it.
                    end(term, Reason::Exit, code, now, &mut after);
                }
            }
        }
        after.run();
    }

    /// Starts the once-a-second tick on the current tokio runtime. Safe to call
    /// again: only one ticker runs.
    pub fn start_ticker(self: &Arc<Self>) {
        {
            let mut state = lock(&self.state);
            if state.ticking {
                return;
            }
            state.ticking = true;
        }
        let hub: Weak<Hub> = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let Some(hub) = hub.upgrade() else { break };
                hub.tick();
            }
        });
    }

    /// What the desktop shows.
    pub fn status(&self) -> TerminalStatus {
        let now = self.clock.now_ms();
        let mut state = lock(&self.state);
        prune(&mut state, now, &self.limits);
        let mut open: Vec<OpenTerminal> = state
            .terminals
            .iter()
            .filter(|(_, t)| !matches!(t.phase, Phase::Ended { .. }))
            .map(|(id, t)| OpenTerminal {
                id: id.clone(),
                device_id: t.owner.device_id.clone(),
                device_name: t.owner.name.clone(),
                project: t.label.clone(),
                opened_unix: t.opened_unix,
                detached: matches!(t.phase, Phase::Detached(_)),
            })
            .collect();
        open.sort_by_key(|t| t.opened_unix);
        let codes = state
            .challenges
            .iter()
            .map(|c| PendingCode {
                device_name: c.owner.name.clone(),
                project: c.label.clone(),
                code: c.code.clone(),
                expires_in_ms: c.expires.saturating_sub(now),
            })
            .collect();
        TerminalStatus { open, codes }
    }

    /// Shells that are not ended, of one device.
    pub fn open_count(&self, device_id: &str) -> usize {
        lock(&self.state)
            .terminals
            .values()
            .filter(|t| t.owner.device_id == device_id && !matches!(t.phase, Phase::Ended { .. }))
            .count()
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Drops expired codes, old lockout records and ended shells nobody read.
fn prune(state: &mut State, now: u64, limits: &Limits) {
    state.challenges.retain(|c| c.expires > now);
    state.failures.retain(|_, f| {
        f.at.retain(|at| now.saturating_sub(*at) < limits.lockout_window_ms);
        f.locked_until > now || !f.at.is_empty()
    });
    state
        .last_code
        .retain(|_, at| now.saturating_sub(*at) < limits.code_gap_ms);
    state.terminals.retain(|_, t| match t.phase {
        Phase::Ended { at, .. } => now.saturating_sub(at) < limits.tombstone_ms,
        _ => true,
    });
}

/// The shell `id` of `owner`. Another device's id looks like an unknown one,
/// and so does a shell that ended longer ago than the tombstone time.
fn owned<'a>(
    state: &'a mut State,
    owner: &Owner,
    id: &str,
    now: u64,
    limits: &Limits,
) -> Result<&'a mut Term, Failure> {
    prune(state, now, limits);
    match state.terminals.get_mut(id) {
        Some(term)
            if term.owner.device_id == owner.device_id
                && term.owner.fingerprint == owner.fingerprint =>
        {
            Ok(term)
        }
        _ => Err(failure(Code::NotFound)),
    }
}

/// A shell that takes input: not ended. Input counts as activity, and a
/// detached shell is attached again by its own device.
fn live(term: &mut Term, now: u64) -> Result<(), Failure> {
    if matches!(term.phase, Phase::Ended { .. }) {
        return Err(failure(Code::NotFound));
    }
    term.phase = Phase::Live;
    term.last_input = now;
    Ok(())
}

/// Ends `term`: kills the shell and its group once the lock is released, and
/// leaves it readable for the tombstone time.
fn end(term: &mut Term, reason: Reason, code: Option<i32>, now: u64, after: &mut After) {
    if matches!(term.phase, Phase::Ended { .. }) {
        return;
    }
    term.phase = Phase::Ended {
        reason,
        at: now,
        code,
    };
    term.out.close();
    after.kill.push(term.handle.clone());
    after.audit.push((
        Lifecycle::TerminalClose,
        term.owner.clone(),
        Some(reason.as_str().into()),
    ));
}

#[cfg(test)]
pub(crate) mod tests;
