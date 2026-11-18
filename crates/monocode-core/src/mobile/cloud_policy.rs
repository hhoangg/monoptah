//! Cloud and remote session policy for remote devices (WS-D4): what a device
//! may see of cloud and remote sessions, and the pure rules the cloud methods
//! share. Nothing here touches the app, the network or the clock (callers pass
//! `Instant`), so every rule is unit tested.
//!
//! * [`scope_catalog`]: strips web-only sessions, projects and fields from a
//!   catalog reply for phones (D1) and removes the markers for web devices.
//! * [`is_non_local_project`]: D9, a project id that names a remote project.
//! * [`WakeLimiter`]: D5, the per-device rate limit of billable calls.
//! * [`sanitize_startup_error`]: D6, the only text of a failed start that may
//!   leave the desktop.
//! * [`validate_start_target`]: H-11, the allowlist of `session.start`'s
//!   `target`.
use super::policy::{Code, DeviceContext, DeviceKind, Failure};
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// The marker the app puts on a catalog session or project that exists only
/// for web devices (a cloud or remote-machine session that the QUIC catalog
/// never listed). [`scope_catalog`] drops those for phones and removes the
/// marker for everyone.
pub const WEB_ONLY: &str = "webOnly";

/// The session card fields only web devices get. [`scope_catalog`] removes
/// them from phone replies.
pub const WEB_CARD_FIELDS: [&str; 3] = ["target", "cloudState", "startup"];

/// Applies the transport scope (D1, architecture.md 6.6) to a catalog reply
/// (`workspace.list`, `sessions.list`).
///
/// The app builds one catalog for every device. For a phone this removes the
/// sessions and projects marked [`WEB_ONLY`] and the web-only fields of the
/// remaining cards, so a phone gets exactly the lists it always did. For a web
/// device it only removes the marker. Only session cards (they carry a
/// `projectId`) and project cards are touched, so other replies pass through.
pub fn scope_catalog(device: &DeviceContext, catalog: &mut Value) {
    let phone = device.kind == DeviceKind::Phone;
    if let Some(list) = catalog.get_mut("sessions").and_then(Value::as_array_mut) {
        if phone {
            list.retain(|card| card.get(WEB_ONLY).is_none());
        }
        for card in list.iter_mut() {
            if let Some(card) = card.as_object_mut().filter(|c| c.contains_key("projectId")) {
                card.remove(WEB_ONLY);
                if phone {
                    for field in WEB_CARD_FIELDS {
                        card.remove(field);
                    }
                }
            }
        }
    }
    if let Some(list) = catalog.get_mut("projects").and_then(Value::as_array_mut) {
        if phone {
            list.retain(|card| card.get(WEB_ONLY).is_none());
        }
        for card in list.iter_mut().filter_map(Value::as_object_mut) {
            card.remove(WEB_ONLY);
        }
    }
}

/// The percent-decoded form of `id` (invalid escapes are kept as they are).
fn percent_decode(id: &str) -> String {
    let bytes = id.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        let pair = (i + 2 < bytes.len() && bytes[i] == b'%')
            .then(|| hex(bytes[i + 1]).zip(hex(bytes[i + 2])))
            .flatten();
        if let Some((high, low)) = pair {
            out.push((high * 16 + low) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `project_id` names a remote-machine project rather than a local
/// one (D9). The app encodes a project's path in its id, and a remote
/// project's path starts with `remote://`, so its id starts with
/// `remote%3A%2F%2F` in either case of the hex digits. The id is decoded
/// first, so any other spelling of the same prefix is caught too. A local path
/// that merely has "remote" in its name is not affected: an encoded absolute
/// path starts with `%2F` or a drive letter.
pub fn is_non_local_project(project_id: &str) -> bool {
    percent_decode(project_id)
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("remote://"))
}

// MARK: rate limit (D5)

/// Wake-causing calls one device may make per rolling hour.
pub const WAKE_LIMIT_PER_HOUR: usize = 6;
/// Wake-causing operations one device may have in flight.
pub const WAKE_CONCURRENT: usize = 2;
/// An in-flight slot is released after this long even when its completion was
/// lost, so a device cannot be locked out for good.
pub const WAKE_SLOT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Reads of `cloud.instances` and `cloud.startOptions` per device and minute.
pub const READ_LIMIT_PER_MINUTE: usize = 30;

const HOUR: Duration = Duration::from_secs(60 * 60);
const MINUTE: Duration = Duration::from_secs(60);

/// A claimed in-flight slot. Pass it to [`WakeLimiter::release`] when the
/// operation ends (instance available, start failed, move finished).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeSlot {
    device: String,
    id: u64,
}

#[derive(Default)]
struct DeviceUse {
    /// Start times of the wake-causing calls in the last hour.
    starts: VecDeque<(u64, Instant)>,
    /// In-flight operations, by slot id.
    slots: Vec<(u64, Instant)>,
    reads: VecDeque<Instant>,
}

/// The per-device limit of billable calls (security.md T16). Time is passed in,
/// never read, so the rules are testable.
#[derive(Default)]
pub struct WakeLimiter {
    devices: HashMap<String, DeviceUse>,
    next: u64,
}

impl WakeLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    fn forget_old(usage: &mut DeviceUse, now: Instant) {
        let age = |at: Instant| now.saturating_duration_since(at);
        while usage.starts.front().is_some_and(|(_, at)| age(*at) >= HOUR) {
            usage.starts.pop_front();
        }
        usage.slots.retain(|(_, at)| age(*at) < WAKE_SLOT_TIMEOUT);
        while usage.reads.front().is_some_and(|at| age(*at) >= MINUTE) {
            usage.reads.pop_front();
        }
    }

    /// Claims a slot for one wake-causing call of `device`. `Busy` when the
    /// device made [`WAKE_LIMIT_PER_HOUR`] such calls in the last hour or has
    /// [`WAKE_CONCURRENT`] in flight. A refused call is not counted.
    pub fn acquire(&mut self, device: &str, now: Instant) -> Result<WakeSlot, Code> {
        let usage = self.devices.entry(device.to_owned()).or_default();
        Self::forget_old(usage, now);
        if usage.starts.len() >= WAKE_LIMIT_PER_HOUR || usage.slots.len() >= WAKE_CONCURRENT {
            return Err(Code::Busy);
        }
        self.next += 1;
        let id = self.next;
        usage.starts.push_back((id, now));
        usage.slots.push((id, now));
        Ok(WakeSlot {
            device: device.to_owned(),
            id,
        })
    }

    /// Ends an operation: its slot is free again, its call still counts for
    /// the hour. A slot released twice, or after its timeout, is a no-op.
    pub fn release(&mut self, slot: &WakeSlot) {
        if let Some(usage) = self.devices.get_mut(&slot.device) {
            usage.slots.retain(|(id, _)| *id != slot.id);
        }
    }

    /// Ends an operation that never started anything (refused after the slot
    /// was claimed): the slot is free and the call does not count either.
    pub fn refund(&mut self, slot: &WakeSlot) {
        if let Some(usage) = self.devices.get_mut(&slot.device) {
            usage.slots.retain(|(id, _)| *id != slot.id);
            usage.starts.retain(|(id, _)| *id != slot.id);
        }
    }

    /// Counts one read of `device`; `Busy` beyond [`READ_LIMIT_PER_MINUTE`]
    /// in the last minute.
    pub fn allow_read(&mut self, device: &str, now: Instant) -> Result<(), Code> {
        let usage = self.devices.entry(device.to_owned()).or_default();
        Self::forget_old(usage, now);
        if usage.reads.len() >= READ_LIMIT_PER_MINUTE {
            return Err(Code::Busy);
        }
        usage.reads.push_back(now);
        Ok(())
    }

    /// Operations of `device` in flight now.
    pub fn in_flight(&mut self, device: &str, now: Instant) -> usize {
        self.devices.get_mut(device).map_or(0, |usage| {
            Self::forget_old(usage, now);
            usage.slots.len()
        })
    }

    /// Wake-causing calls of `device` in the last hour.
    pub fn calls_this_hour(&mut self, device: &str, now: Instant) -> usize {
        self.devices.get_mut(device).map_or(0, |usage| {
            Self::forget_old(usage, now);
            usage.starts.len()
        })
    }

    /// Forgets a device that left, so its history does not outlive its grant.
    pub fn forget(&mut self, device: &str) {
        self.devices.remove(device);
    }
}

static LIMITER: LazyLock<Mutex<WakeLimiter>> = LazyLock::new(|| Mutex::new(WakeLimiter::new()));

/// The process-wide limiter the cloud methods share, locked. A poisoned lock
/// is recovered: the limiter holds counters only. Never hold the guard across
/// a call into the app.
pub fn limiter() -> MutexGuard<'static, WakeLimiter> {
    LIMITER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Claims a wake slot on the process-wide limiter, as a failure ready to
/// return (`busy`).
pub fn acquire_wake(device: &str) -> Result<WakeSlot, Failure> {
    limiter()
        .acquire(device, Instant::now())
        .map_err(Code::failure)
}

/// Releases a slot on the process-wide limiter.
pub fn release_wake(slot: &WakeSlot) {
    limiter().release(slot);
}

/// Counts a read on the process-wide limiter.
pub fn allow_read(device: &str) -> Result<(), Failure> {
    limiter()
        .allow_read(device, Instant::now())
        .map_err(Code::failure)
}

/// Empties the process-wide limiter. Tests that share it call this first.
pub fn reset_limiter() {
    *limiter() = WakeLimiter::new();
}

// MARK: startup error (D6)

/// The longest `startup.error` that leaves the desktop, in characters.
pub const STARTUP_ERROR_MAX_CHARS: usize = 200;

/// What a failed start shows when nothing of its error is safe to show.
const STARTUP_ERROR_FALLBACK: &str = "The start failed";

fn token_like(word: &str) -> bool {
    const PREFIXES: [&str; 10] = [
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "ghr_",
        "github_pat_",
        "dckr_pat_",
        "sk-",
        "xox",
        "eyj",
    ];
    let lower = word.to_ascii_lowercase();
    if PREFIXES
        .iter()
        .any(|p| lower.starts_with(p) && word.len() > p.len() + 3)
    {
        return true;
    }
    // A long run of letters and digits with no separators: a key or a hash.
    let alnum = word
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .count();
    word.chars().count() >= 20
        && alnum == word.chars().count()
        && word.chars().any(|c| c.is_ascii_digit())
}

/// Whether one whitespace-separated word of an error must not leave the
/// desktop: a path, URL, host, address, option, variable assignment, account
/// or token.
fn unsafe_word(word: &str) -> bool {
    let core = word.trim_matches(|c: char| {
        matches!(
            c,
            '\'' | '"'
                | '`'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | ';'
                | ':'
                | '.'
                | '!'
                | '?'
        )
    });
    if core.is_empty() {
        return false;
    }
    let has_dot_inside = core
        .char_indices()
        .any(|(i, c)| c == '.' && i > 0 && i + 1 < core.len());
    core.contains("://")
        || core.contains('/')
        || core.contains('\\')
        || core.starts_with('~')
        || core.starts_with('-') && core.len() > 1
        || core.starts_with('$')
        || core.starts_with('%')
        || core.contains('=')
        || core.contains('@')
        || has_dot_inside
        || token_like(core)
        || word.contains('`')
}

/// Commands the desktop runs: an error that quotes one is cut where it starts.
const COMMANDS: [&str; 8] = ["gh", "sbx", "ssh", "docker", "git", "scp", "rsync", "sh"];
/// Words that follow a command name in a quoted command line.
const COMMAND_VERBS: [&str; 8] = [
    "codespace",
    "sandbox",
    "run",
    "create",
    "clone",
    "push",
    "pull",
    "exec",
];

/// The text of a failed start that may leave the desktop (D6): every word that
/// looks like a path, URL, host, address, option, variable, account or token is
/// dropped, an error that quotes a command line is cut where the command
/// starts, whitespace is collapsed and the result is at most
/// [`STARTUP_ERROR_MAX_CHARS`] characters. Never empty.
pub fn sanitize_startup_error(raw: &str) -> String {
    let words: Vec<&str> = raw.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let starts_command = COMMANDS.contains(word)
            && words
                .get(i + 1)
                .is_some_and(|next| next.starts_with('-') || COMMAND_VERBS.contains(next));
        if starts_command {
            break;
        }
        if !unsafe_word(word) {
            kept.push(word);
        }
    }
    let text = kept.join(" ");
    if text.is_empty() {
        return STARTUP_ERROR_FALLBACK.into();
    }
    if text.chars().count() <= STARTUP_ERROR_MAX_CHARS {
        return text;
    }
    let mut cut: String = text.chars().take(STARTUP_ERROR_MAX_CHARS - 3).collect();
    cut.truncate(cut.trim_end().len());
    cut.push_str("...");
    cut
}

// MARK: start target (H-11)

/// The backend a remote start asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartKind {
    Codespace,
    Sandbox,
}

/// Which instance a remote start runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartInstance {
    /// Create one.
    New,
    /// An existing one, by the name `cloud.instances` listed.
    Named(String),
}

/// The `target` of `session.start`, validated against the allowlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartTarget {
    pub kind: StartKind,
    pub instance: StartInstance,
    pub branch: Option<String>,
    /// Codespace only: a machine from `cloud.startOptions`.
    pub machine: Option<String>,
    /// Sandbox only: a size preset from `cloud.startOptions`.
    pub size: Option<String>,
}

const START_TARGET_KEYS: [&str; 5] = ["kind", "instance", "branch", "machine", "size"];
const START_TEXT_MAX_CHARS: usize = 200;

fn start_text(value: &Value) -> Result<String, Code> {
    let text = value.as_str().ok_or(Code::Forbidden)?;
    if text.is_empty()
        || text.chars().count() > START_TEXT_MAX_CHARS
        || text.chars().any(char::is_control)
    {
        return Err(Code::Forbidden);
    }
    Ok(text.to_owned())
}

/// Checks the `target` of a remote `session.start`. Only `kind`
/// (`codespace` or `sandbox`), `instance` (`"new"` or a name), `branch`,
/// `machine` (codespace) and `size` (sandbox) are accepted. `kit`, `template`,
/// `image`, `cpus`, `memory`, `ttl` and every other key give `Forbidden`, as do
/// values of the wrong type, so the client can never choose what the desktop
/// pays for or sends to the instance.
pub fn validate_start_target(target: &Value) -> Result<StartTarget, Code> {
    let map: &Map<String, Value> = target.as_object().ok_or(Code::Forbidden)?;
    if map
        .keys()
        .any(|key| !START_TARGET_KEYS.contains(&key.as_str()))
    {
        return Err(Code::Forbidden);
    }
    let kind = match map.get("kind").and_then(Value::as_str) {
        Some("codespace") => StartKind::Codespace,
        Some("sandbox") => StartKind::Sandbox,
        _ => return Err(Code::Forbidden),
    };
    let instance = match map.get("instance") {
        None => StartInstance::New,
        Some(value) => match start_text(value)?.as_str() {
            "new" => StartInstance::New,
            name => StartInstance::Named(name.to_owned()),
        },
    };
    let optional = |key: &str| map.get(key).map(start_text).transpose();
    let branch = optional("branch")?;
    let machine = optional("machine")?;
    let size = optional("size")?;
    if machine.is_some() && kind != StartKind::Codespace
        || size.is_some() && kind != StartKind::Sandbox
    {
        return Err(Code::Forbidden);
    }
    Ok(StartTarget {
        kind,
        instance,
        branch,
        machine,
        size,
    })
}
