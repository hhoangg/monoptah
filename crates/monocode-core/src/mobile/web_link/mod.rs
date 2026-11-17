//! The web link: an outbound relay client that serves paired browsers through
//! the same host as the QUIC phones. It dials the relay, binds no port and
//! never starts the port-mapping router.
pub(crate) mod channel;
pub mod frame;
pub mod grants;
#[cfg(test)]
mod live_tests;
pub mod noise;
mod pacer;
pub mod relay;
pub mod session;
#[cfg(test)]
mod tests;

use super::host::MobileHost;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

/// Where the web link stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebState {
    /// Not running: switched off, or no web identity was paired yet.
    #[default]
    Off,
    Connecting,
    Connected,
    /// The relay is unreachable or closed the socket; the client retries.
    Offline,
    /// The relay refused the desktop for good. The desktop shows "Web app:
    /// access revoked, pair again".
    Revoked,
}

/// What Settings shows about the web link. Never carries the token.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebStatus {
    pub enabled: bool,
    pub state: WebState,
    pub error: Option<String>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<(WebState, Option<String>)>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pending: grants::PendingGrants,
    generation: AtomicU64,
}

/// The web link's client task, status and pending grants, owned by the host.
#[derive(Clone, Default)]
pub struct WebLink {
    shared: Arc<Shared>,
}

impl WebLink {
    pub(crate) fn status(&self, enabled: bool) -> WebStatus {
        let (state, error) = self
            .shared
            .state
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or((WebState::Off, None));
        WebStatus {
            enabled,
            state,
            error,
        }
    }

    pub(crate) fn set(&self, state: WebState, error: Option<String>) {
        if let Ok(mut guard) = self.shared.state.lock() {
            *guard = (state, error);
        }
    }

    pub(crate) fn pending(&self) -> &grants::PendingGrants {
        &self.shared.pending
    }

    fn is_running(&self) -> bool {
        self.shared
            .task
            .lock()
            .is_ok_and(|task| task.as_ref().is_some_and(|task| !task.is_finished()))
    }

    /// Aborts the client (its sessions and watches go with it) and reports Off.
    pub(crate) fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut task) = self.shared.task.lock() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
        self.set(WebState::Off, None);
    }
}

/// Starts the outbound relay client for `host` when the web link is switched
/// on and a web identity exists. Uses the host's runtime, binds nothing.
pub fn start(host: &MobileHost) {
    sync(host);
}

/// Brings the client in line with the saved `web_enabled` flag.
pub(crate) fn sync(host: &MobileHost) {
    sync_with(host, relay::Timing::default());
}

pub(crate) fn sync_with(host: &MobileHost, timing: relay::Timing) {
    let link = host.web_link().clone();
    if !host.web_enabled() || host.is_shut_down() {
        link.stop();
        return;
    }
    if link.is_running() {
        return;
    }
    let runtime = match host.runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            link.set(WebState::Off, Some(error));
            return;
        }
    };
    let generation = link.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let task_host = host.clone();
    let task_link = link.clone();
    let handle = runtime.spawn(async move {
        // The sealed store is a database read: keep it off the async workers.
        let loader = task_host.clone();
        let loaded = tokio::task::spawn_blocking(move || loader.load_web_identity()).await;
        let identity = match loaded {
            Ok(Ok(Some(identity))) => identity,
            Ok(Ok(None)) => {
                task_link.set(WebState::Off, None);
                return;
            }
            Ok(Err(error)) => {
                task_link.set(WebState::Off, Some(error));
                return;
            }
            Err(_) => {
                task_link.set(WebState::Off, Some("Web link could not start".into()));
                return;
            }
        };
        if task_link.shared.generation.load(Ordering::SeqCst) != generation
            || !task_host.web_enabled()
        {
            return;
        }
        relay::run(task_host, task_link, identity, timing).await;
    });
    if let Ok(mut task) = link.shared.task.lock() {
        if let Some(old) = task.replace(handle) {
            old.abort();
        }
    };
}

/// Re-sends the current state (capabilities, selection) on every live web
/// channel, after a policy toggle changes. Nothing is connected yet.
#[allow(dead_code)]
pub fn resend_state() {}
