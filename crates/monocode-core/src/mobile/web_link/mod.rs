//! The web link: an outbound relay client that serves paired browsers through
//! the same host as the QUIC phones. Wave 0 declares the modules only; each is
//! filled by its own task.
pub mod frame;
pub mod grants;
pub mod noise;
pub mod relay;
pub mod session;

use super::host::MobileHost;

/// Starts the outbound relay client for `host`. Nothing to start yet.
#[allow(dead_code)]
pub fn start(_host: &MobileHost) {}

/// Re-sends the current state (capabilities, selection) on every live web
/// channel, after a policy toggle changes. Nothing is connected yet.
#[allow(dead_code)]
pub fn resend_state() {}
