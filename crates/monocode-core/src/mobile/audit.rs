//! Audit trail of what remote devices did (security.md section 7). The stub
//! records nothing yet; the fields are fixed so callers can be written now.
use super::policy::{Code, DeviceContext};

/// How a request reached the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Quic,
    #[allow(dead_code)]
    Web,
}

/// What became of the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    #[allow(dead_code)]
    Denied(Code),
    #[allow(dead_code)]
    Error,
}

/// One audited event. Parameters and content are never part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEvent {
    /// The first characters of the device's key fingerprint.
    pub fingerprint_prefix: String,
    pub transport: Transport,
    pub method: String,
    pub project_id: Option<String>,
    pub outcome: Outcome,
    pub command_id: Option<String>,
}

/// Records `event` for `device`. A no-op until the log exists.
pub fn record(_device: &DeviceContext, _event: &AuditEvent) {}
