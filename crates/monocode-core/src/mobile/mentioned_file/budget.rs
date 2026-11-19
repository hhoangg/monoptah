//! The per-device budget of `chat.mentionedFile`: calls and bytes served over a sliding window.
//!
//! A paired device may read whatever the assistant mentioned, so the budget bounds what a
//! compromised device can pull out of that narrow window, and what a looping page can cost.
use std::{
    collections::{HashMap, VecDeque},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use super::super::policy::Code;

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

/// Adds the bytes of a reply on the process-wide limiter.
pub fn served(device: &str, bytes: u64) {
    limiter().served(device, Instant::now(), bytes);
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
    fn a_forgotten_device_starts_fresh() {
        let mut limiter = Limiter::new();
        let now = Instant::now();
        limiter.served("d", now, BYTES_PER_WINDOW);
        limiter.forget("d");
        assert_eq!(limiter.admit("d", now), Ok(()));
    }
}
