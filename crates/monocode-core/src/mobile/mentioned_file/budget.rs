//! The per-device budget of `chat.mentionedFile`: calls and bytes served over a sliding window.
//!
//! A paired device may read whatever the assistant mentioned, so the budget bounds what a
//! compromised device can pull out of that narrow window, and what a looping page can cost.
use std::{
    collections::{HashMap, VecDeque},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use super::super::policy::{Code, Failure};

/// Calls one device may make in [`CALL_WINDOW`].
pub const CALLS_PER_WINDOW: usize = 120;
pub const CALL_WINDOW: Duration = Duration::from_secs(60);
/// Bytes one device may be served in [`BYTE_WINDOW`].
pub const BYTES_PER_WINDOW: u64 = 64 * 1024 * 1024;
pub const BYTE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Devices tracked at once; the oldest idle ones go first.
const MAX_DEVICES: usize = 256;

#[derive(Default)]
struct Usage {
    calls: VecDeque<Instant>,
    bytes: VecDeque<(Instant, u64)>,
}

/// Time is passed in, never read, so the rules are testable.
#[derive(Default)]
pub struct Limiter {
    devices: HashMap<String, Usage>,
}

impl Limiter {
    pub fn new() -> Self {
        Self::default()
    }

    fn forget_old(usage: &mut Usage, now: Instant) {
        let age = |at: Instant| now.saturating_duration_since(at);
        while usage
            .calls
            .front()
            .is_some_and(|at| age(*at) >= CALL_WINDOW)
        {
            usage.calls.pop_front();
        }
        while usage
            .bytes
            .front()
            .is_some_and(|(at, _)| age(*at) >= BYTE_WINDOW)
        {
            usage.bytes.pop_front();
        }
    }

    /// Counts one call of `device`. `Busy` when it made [`CALLS_PER_WINDOW`] calls in the last
    /// minute or was already served [`BYTES_PER_WINDOW`] bytes in the last ten. A refused call
    /// is not counted.
    pub fn admit(&mut self, device: &str, now: Instant) -> Result<(), Code> {
        if self.devices.len() >= MAX_DEVICES && !self.devices.contains_key(device) {
            self.devices.retain(|_, usage| {
                Self::forget_old(usage, now);
                !usage.calls.is_empty() || !usage.bytes.is_empty()
            });
        }
        let usage = self.devices.entry(device.to_owned()).or_default();
        Self::forget_old(usage, now);
        let served: u64 = usage.bytes.iter().map(|(_, n)| n).sum();
        if usage.calls.len() >= CALLS_PER_WINDOW || served >= BYTES_PER_WINDOW {
            return Err(Code::Busy);
        }
        usage.calls.push_back(now);
        Ok(())
    }

    /// Counts `bytes` against the device's byte window before the work that will spend them starts, so parallel calls
    /// cannot together pass the window. `Busy` when they would not fit; nothing is counted then.
    pub fn reserve(&mut self, device: &str, now: Instant, bytes: u64) -> Result<(), Code> {
        let usage = self.devices.entry(device.to_owned()).or_default();
        Self::forget_old(usage, now);
        let served: u64 = usage.bytes.iter().map(|(_, n)| n).sum();
        if served.saturating_add(bytes) > BYTES_PER_WINDOW {
            return Err(Code::Busy);
        }
        if bytes > 0 {
            usage.bytes.push_back((now, bytes));
        }
        Ok(())
    }

    /// Takes back a [`Limiter::reserve`] whose work spent nothing (the object was already there, or the call failed).
    pub fn refund(&mut self, device: &str, bytes: u64) {
        if let Some(usage) = self.devices.get_mut(device) {
            if let Some(at) = usage.bytes.iter().rposition(|(_, n)| *n == bytes) {
                usage.bytes.remove(at);
            }
        }
    }

    /// How long until a call that wants `bytes` more could pass the windows (a call counts for one call and at least one
    /// byte): the time the oldest entries in its way still have to live. Zero when nothing blocks it.
    pub fn retry_after(&mut self, device: &str, now: Instant, bytes: u64) -> Duration {
        let Some(usage) = self.devices.get_mut(device) else {
            return Duration::ZERO;
        };
        Self::forget_old(usage, now);
        let left = |at: Instant, window: Duration| (at + window).saturating_duration_since(now);
        let mut wait = Duration::ZERO;
        if usage.calls.len() >= CALLS_PER_WINDOW {
            wait = wait.max(left(
                usage.calls[usage.calls.len() - CALLS_PER_WINDOW],
                CALL_WINDOW,
            ));
        }
        let need = bytes.max(1);
        let mut served: u64 = usage.bytes.iter().map(|(_, n)| n).sum();
        if served.saturating_add(need) > BYTES_PER_WINDOW {
            // Oldest first: the call fits once enough of them have aged out; if even all of them do not make room, the
            // call is larger than the window and the whole window is the honest answer.
            let mut freed_at = BYTE_WINDOW;
            for (at, n) in &usage.bytes {
                served = served.saturating_sub(*n);
                if served.saturating_add(need) <= BYTES_PER_WINDOW {
                    freed_at = left(*at, BYTE_WINDOW);
                    break;
                }
            }
            wait = wait.max(freed_at);
        }
        wait
    }

    /// Adds the bytes of a reply to the device's tally.
    pub fn served(&mut self, device: &str, now: Instant, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let usage = self.devices.entry(device.to_owned()).or_default();
        usage.bytes.push_back((now, bytes));
    }

    /// Forgets a device that left, so its history does not outlive its grant.
    pub fn forget(&mut self, device: &str) {
        self.devices.remove(device);
    }
}

static LIMITER: LazyLock<Mutex<Limiter>> = LazyLock::new(|| Mutex::new(Limiter::new()));

fn limiter() -> std::sync::MutexGuard<'static, Limiter> {
    LIMITER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Counts a call on the process-wide limiter: `Busy` over budget.
pub fn admit(device: &str) -> Result<(), Code> {
    limiter().admit(device, Instant::now())
}

/// The `busy` refusal of a call that wants `bytes` more, with the seconds until it could pass (at least 1, at most one
/// byte window), so a client waits that long instead of asking again at once.
pub fn busy(device: &str, bytes: u64) -> Failure {
    let wait = limiter().retry_after(device, Instant::now(), bytes);
    let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    Code::Busy
        .failure()
        .retry_after(seconds.clamp(1, BYTE_WINDOW.as_secs()))
}

/// Adds the bytes of a reply on the process-wide limiter.
pub fn served(device: &str, bytes: u64) {
    limiter().served(device, Instant::now(), bytes);
}

/// [`Limiter::reserve`] on the process-wide limiter.
pub fn reserve(device: &str, bytes: u64) -> Result<(), Code> {
    limiter().reserve(device, Instant::now(), bytes)
}

/// [`Limiter::refund`] on the process-wide limiter.
pub fn refund(device: &str, bytes: u64) {
    limiter().refund(device, bytes);
}

/// Forgets a device on the process-wide limiter.
pub fn forget_device(device: &str) {
    limiter().forget(device);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_121st_call_in_a_minute_is_busy_and_a_new_minute_resets() {
        let mut limiter = Limiter::new();
        let start = Instant::now();
        for i in 0..CALLS_PER_WINDOW {
            assert_eq!(
                limiter.admit("d", start + Duration::from_millis(i as u64)),
                Ok(())
            );
        }
        assert_eq!(
            limiter.admit("d", start + Duration::from_secs(1)),
            Err(Code::Busy)
        );
        // Another device is not affected.
        assert_eq!(limiter.admit("other", start), Ok(()));
        assert_eq!(
            limiter.admit("d", start + CALL_WINDOW + Duration::from_secs(1)),
            Ok(())
        );
    }

    #[test]
    fn the_byte_budget_closes_the_door_until_the_window_passes() {
        let mut limiter = Limiter::new();
        let start = Instant::now();
        assert_eq!(limiter.admit("d", start), Ok(()));
        limiter.served("d", start, BYTES_PER_WINDOW);
        assert_eq!(
            limiter.admit("d", start + Duration::from_secs(5)),
            Err(Code::Busy)
        );
        assert_eq!(
            limiter.admit("d", start + BYTE_WINDOW + Duration::from_secs(1)),
            Ok(())
        );
    }

    #[test]
    fn a_reservation_counts_at_once_and_a_refund_gives_it_back() {
        let mut limiter = Limiter::new();
        let now = Instant::now();
        let half = BYTES_PER_WINDOW / 2;
        assert_eq!(limiter.reserve("d", now, half), Ok(()));
        assert_eq!(limiter.reserve("d", now, half), Ok(()));
        // Two calls in flight have spent the window between them.
        assert_eq!(limiter.reserve("d", now, 1), Err(Code::Busy));
        limiter.refund("d", half);
        assert_eq!(limiter.reserve("d", now, half), Ok(()));
        assert_eq!(limiter.reserve("d", now, 1), Err(Code::Busy));
        // Another device is not affected, and the window passing frees the bytes.
        assert_eq!(limiter.reserve("other", now, half), Ok(()));
        assert_eq!(
            limiter.reserve("d", now + BYTE_WINDOW + Duration::from_secs(1), half),
            Ok(())
        );
    }

    #[test]
    fn a_refusal_says_how_long_until_the_call_could_pass() {
        let mut limiter = Limiter::new();
        let start = Instant::now();
        // Nothing in the way: no wait.
        assert_eq!(limiter.retry_after("d", start, 1), Duration::ZERO);
        // The call window: the 121st call waits for the oldest one to age out.
        for i in 0..CALLS_PER_WINDOW {
            limiter
                .admit("d", start + Duration::from_secs(i as u64 % 10))
                .unwrap();
        }
        let now = start + Duration::from_secs(20);
        assert_eq!(
            limiter.retry_after("d", now, 1),
            CALL_WINDOW - Duration::from_secs(20)
        );
        // The byte window: room for 10 MiB comes when the first reservation (taken at `start`) ages out.
        let mut bytes = Limiter::new();
        bytes.reserve("d", start, BYTES_PER_WINDOW - 5).unwrap();
        bytes
            .reserve("d", start + Duration::from_secs(100), 5)
            .unwrap();
        let later = start + Duration::from_secs(200);
        assert_eq!(
            bytes.retry_after("d", later, 10),
            BYTE_WINDOW - Duration::from_secs(200)
        );
        // A call larger than the whole window never fits: the whole window is the answer.
        assert_eq!(
            bytes.retry_after("d", later, BYTES_PER_WINDOW + 1),
            BYTE_WINDOW
        );
    }

    #[test]
    fn the_busy_failure_of_a_full_window_carries_retry_after() {
        let device = "retry-after-device";
        served(device, BYTES_PER_WINDOW);
        let failure = busy(device, 1);
        assert_eq!(failure.code.as_deref(), Some("busy"));
        let seconds = failure.retry_after.expect("a hint");
        assert!((1..=BYTE_WINDOW.as_secs()).contains(&seconds), "{seconds}");
        forget_device(device);
    }

    #[test]
    fn a_forgotten_device_starts_fresh() {
        let mut limiter = Limiter::new();
        let now = Instant::now();
        limiter.served("d", now, BYTES_PER_WINDOW);
        limiter.forget("d");
        assert_eq!(limiter.admit("d", now), Ok(()));
    }
}
