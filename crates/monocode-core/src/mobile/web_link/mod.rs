//! The web link: an outbound relay client that serves paired browsers through
//! the same host as the QUIC phones. It dials the relay, binds no port and
//! never starts the port-mapping router.
pub(crate) mod channel;
pub mod desktop_api;
pub mod frame;
pub mod grants;
#[cfg(test)]
mod live_tests;
pub mod noise;
mod pacer;
pub mod presence;
pub mod relay;
pub mod session;
#[cfg(test)]
mod tests;

use super::host::{Device, MobileHost};
use super::identity_store::WebIdentity;
use super::policy::Permission;
use session::SessionKey;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use zeroize::Zeroizing;

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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WebStatus {
    pub enabled: bool,
    pub state: WebState,
    pub error: Option<String>,
    /// A web identity is sealed (cached: no database read here).
    pub paired: bool,
    /// The web account's handle.
    pub account: Option<String>,
    pub desktop_id: Option<String>,
    /// This desktop's own key fingerprint (64 lowercase hex).
    pub fingerprint: Option<String>,
    /// The web server's HTTP base, `https://HOST[:port]`.
    pub host: Option<String>,
    /// Devices the server lists as waiting for the desktop's confirmation.
    pub pending: Vec<PendingDevice>,
    /// Confirmed devices asking for a higher permission.
    pub requests: Vec<PermissionRequest>,
    /// The paired web devices (rows of the desktop's own list).
    pub devices: Vec<Device>,
    /// The last widenings of a device's selection, oldest first (at most 20).
    pub access_changes: Vec<AccessChange>,
}

/// A web device widened what it may see: it is shown to the owner in Settings
/// and as a notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessChange {
    /// Unix seconds.
    pub at: u64,
    pub device_id: String,
    pub device_name: String,
    /// "<device> now has access to <names>".
    pub text: String,
}

/// How many access changes are kept.
const MAX_ACCESS_CHANGES: usize = 20;

/// A device waiting for the desktop's confirmation. `name` is cleaned text
/// from an untrusted source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDevice {
    pub fingerprint: String,
    pub name: String,
    pub server_device_id: String,
    /// A pending Noise session of this device is connected right now.
    pub online: bool,
}

/// A confirmed web device asking for a higher permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequest {
    pub device_id: String,
    pub server_device_id: String,
    pub name: String,
    pub current: Permission,
    pub requested: Permission,
}

/// The pairing a user can start from Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairTicket {
    pub user_code: String,
    pub confirmation_code: String,
    /// `https://HOST/pair/<userCode>#fp=<fingerprint>`: what the QR carries.
    pub link: String,
    /// This desktop's key fingerprint (64 lowercase hex).
    pub fingerprint: String,
    /// Unix seconds.
    pub expires_at: u64,
    /// Seconds between polls.
    pub interval: u64,
}

/// One answer of `MobileHost::web_pair_poll`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairPoll {
    Pending,
    Approved { account: String },
    Denied,
    Expired,
}

/// What the host asks the running relay loop to do with live sessions.
pub(crate) enum LinkCommand {
    /// Move the pending sessions of `fingerprint` to ready.
    Confirm {
        fingerprint: String,
        device: Box<Device>,
    },
    /// End every session of `fingerprint` (denied while pending).
    Deny { fingerprint: String },
    /// End every session of `fingerprint` (revoked).
    Revoke { fingerprint: String },
    /// Send a fresh `state` on every live session of `fingerprint`.
    Resend { fingerprint: String },
    /// Send a fresh `state` on every ready session (a policy toggle changed).
    ResendAll,
    /// Fetch the server's grants again (a report was answered "gone").
    SyncGrants,
}

/// The identity facts Settings shows, cached so a refresh reads no database.
#[derive(Clone)]
struct Facts {
    desktop_id: String,
    account_handle: Option<String>,
    desktop_name: Option<String>,
    host: Option<String>,
    fingerprint: String,
}

/// A pairing in progress: in memory only, wiped on drop.
pub(crate) struct Pairing {
    pub(crate) id: u64,
    pub(crate) device_code: Zeroizing<String>,
    pub(crate) verifier: Zeroizing<[u8; 32]>,
    pub(crate) private: Zeroizing<[u8; 32]>,
    pub(crate) public: [u8; 32],
    pub(crate) base: String,
    /// The machine name sent to the server, kept as the desktop name devices see.
    pub(crate) machine_name: String,
    /// The server asked for a slower poll: no request before this instant.
    pub(crate) not_before: Option<std::time::Instant>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<(WebState, Option<String>)>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pending: grants::PendingGrants,
    generation: AtomicU64,
    facts: Mutex<Option<Facts>>,
    pairing: Mutex<Option<Pairing>>,
    pairing_seq: AtomicU64,
    /// The pending Noise sessions that are connected, with their fingerprint.
    live: Mutex<HashMap<SessionKey, String>>,
    /// The running loop's command channel, tagged with its generation.
    commands: Mutex<Option<(u64, UnboundedSender<LinkCommand>)>>,
    requests: Mutex<Vec<PermissionRequest>>,
    access_changes: Mutex<Vec<AccessChange>>,
    /// Requests the user denied, by device row id, with the permission that
    /// was asked: they stay hidden until the server asks for something else.
    hidden: Mutex<HashMap<String, Permission>>,
    sync: grants::SyncGate,
    /// What each connected web tab shows (memory only).
    presence: Mutex<presence::PresenceTable>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The web link's client task, status and pending grants, owned by the host.
#[derive(Clone, Default)]
pub struct WebLink {
    shared: Arc<Shared>,
}

impl WebLink {
    /// Locks only, no I/O: Settings calls it every second. `devices` are the
    /// host's own rows.
    pub(crate) fn status(&self, enabled: bool, devices: Vec<Device>) -> WebStatus {
        let (state, error) = self
            .shared
            .state
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or((WebState::Off, None));
        let facts = lock(&self.shared.facts).clone();
        let online: Vec<String> = lock(&self.shared.live).values().cloned().collect();
        let pending = self
            .shared
            .pending
            .list()
            .into_iter()
            .map(|(fingerprint, grant)| PendingDevice {
                online: online.contains(&fingerprint),
                fingerprint,
                name: grant.name,
                server_device_id: grant.server_device_id,
            })
            .collect();
        WebStatus {
            enabled,
            state,
            error,
            paired: facts.is_some(),
            account: facts.as_ref().and_then(|f| f.account_handle.clone()),
            desktop_id: facts.as_ref().map(|f| f.desktop_id.clone()),
            fingerprint: facts.as_ref().map(|f| f.fingerprint.clone()),
            host: facts.as_ref().and_then(|f| f.host.clone()),
            pending,
            requests: lock(&self.shared.requests).clone(),
            devices,
            access_changes: lock(&self.shared.access_changes).clone(),
        }
    }

    /// Keeps a widening for Settings and the notification, newest last.
    pub(crate) fn record_access_change(&self, change: AccessChange) {
        let mut list = lock(&self.shared.access_changes);
        list.push(change);
        let extra = list.len().saturating_sub(MAX_ACCESS_CHANGES);
        list.drain(..extra);
    }

    /// Remembers what Settings shows of a sealed identity.
    pub(crate) fn remember(&self, identity: &WebIdentity) {
        let host = desktop_api::base_url(&identity.relay_url, cfg!(debug_assertions)).ok();
        *lock(&self.shared.facts) = Some(Facts {
            desktop_id: identity.desktop_id.clone(),
            account_handle: identity.account_handle.clone(),
            desktop_name: identity.desktop_name.clone(),
            host,
            fingerprint: noise::fingerprint(&identity.static_public),
        });
    }

    /// Forgets everything held in memory about an identity and its devices.
    pub(crate) fn forget(&self) {
        *lock(&self.shared.facts) = None;
        *lock(&self.shared.pairing) = None;
        lock(&self.shared.live).clear();
        lock(&self.shared.presence).clear();
        lock(&self.shared.requests).clear();
        lock(&self.shared.access_changes).clear();
        lock(&self.shared.hidden).clear();
        self.shared.pending.clear();
    }

    pub(crate) fn forget_facts(&self) {
        *lock(&self.shared.facts) = None;
    }

    /// The name devices see for this desktop: the machine name chosen at
    /// pairing, or the host name for an identity sealed before it was kept.
    pub(crate) fn desktop_name(&self) -> String {
        lock(&self.shared.facts)
            .as_ref()
            .and_then(|f| f.desktop_name.clone())
            .unwrap_or_else(crate::mobile::host::name)
    }

    pub(crate) fn is_paired(&self) -> bool {
        lock(&self.shared.facts).is_some()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::SeqCst)
    }

    pub(crate) fn sync_gate(&self) -> &grants::SyncGate {
        &self.shared.sync
    }

    pub(crate) fn next_pairing_id(&self) -> u64 {
        self.shared.pairing_seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(crate) fn pairing(&self) -> std::sync::MutexGuard<'_, Option<Pairing>> {
        lock(&self.shared.pairing)
    }

    pub(crate) fn live_insert(&self, key: SessionKey, fingerprint: String) {
        lock(&self.shared.live).insert(key, fingerprint);
    }

    pub(crate) fn live_remove(&self, key: &SessionKey) {
        lock(&self.shared.live).remove(key);
        lock(&self.shared.presence).remove(key);
    }

    /// Records what a tab reports it is looking at (`None`: nothing).
    pub(crate) fn presence_set(
        &self,
        key: SessionKey,
        device_id: &str,
        target: Option<presence::Target>,
    ) {
        lock(&self.shared.presence).set(key, device_id, target, presence::clock_ms());
    }

    /// Whether a tab of the web device `device_id` shows `target` right now.
    pub(crate) fn presence_is_viewing(&self, device_id: &str, target: &presence::Target) -> bool {
        lock(&self.shared.presence).is_viewing(device_id, target, presence::clock_ms())
    }

    pub(crate) fn live_clear(&self) {
        lock(&self.shared.live).clear();
        lock(&self.shared.presence).clear();
    }

    /// A fresh command channel for a running relay loop; the earlier one, if
    /// any, is replaced.
    pub(crate) fn install_commands(&self, generation: u64) -> UnboundedReceiver<LinkCommand> {
        let (sender, receiver) = unbounded_channel();
        *lock(&self.shared.commands) = Some((generation, sender));
        receiver
    }

    /// Ends a loop's own channel and live set. A loop of an older generation
    /// (aborted by a quick off and on) must not clear the new loop's.
    pub(crate) fn end_run(&self, generation: u64) {
        let mut commands = lock(&self.shared.commands);
        if commands.as_ref().is_some_and(|(g, _)| *g == generation) {
            *commands = None;
            drop(commands);
            self.live_clear();
        }
    }

    pub(crate) fn clear_commands(&self) {
        *lock(&self.shared.commands) = None;
    }

    /// Hands a command to the running relay loop. With no loop running there
    /// are no live sessions to act on, so it is dropped.
    pub(crate) fn command(&self, command: LinkCommand) {
        if let Some((_, sender)) = lock(&self.shared.commands).as_ref() {
            let _ = sender.send(command);
        }
    }

    pub(crate) fn set_requests(&self, requests: Vec<PermissionRequest>) {
        // A denied request stays hidden while the server asks for the same;
        // once it asks for something else (or nothing) the hiding ends.
        let mut hidden = lock(&self.shared.hidden);
        hidden.retain(|id, asked| {
            requests
                .iter()
                .any(|r| &r.device_id == id && r.requested == *asked)
        });
        let still: Vec<PermissionRequest> = requests
            .into_iter()
            .filter(|request| hidden.get(&request.device_id) != Some(&request.requested))
            .collect();
        drop(hidden);
        *lock(&self.shared.requests) = still;
    }

    pub(crate) fn request_for(&self, device_id: &str) -> Option<PermissionRequest> {
        lock(&self.shared.requests)
            .iter()
            .find(|request| request.device_id == device_id)
            .cloned()
    }

    /// Hides `request` until the server's `requestedPermission` changes.
    pub(crate) fn hide_request(&self, request: &PermissionRequest) {
        lock(&self.shared.hidden).insert(request.device_id.clone(), request.requested);
        lock(&self.shared.requests).retain(|r| r.device_id != request.device_id);
    }

    pub(crate) fn drop_request(&self, device_id: &str) {
        lock(&self.shared.requests).retain(|r| r.device_id != device_id);
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
        self.clear_commands();
        self.live_clear();
        self.set(WebState::Off, None);
    }
}

/// The app's mobile host, registered by [`start`] (one host per app; a later
/// `start` replaces it).
static ATTACHED: Mutex<Option<MobileHost>> = Mutex::new(None);

/// The app's mobile host as registered by [`start`], so the shell's web link
/// bridge can reach it. `None` until `start` has run.
pub fn attached_host() -> Option<MobileHost> {
    ATTACHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Starts the outbound relay client for `host` when the web link is switched
/// on and a web identity exists. Uses the host's runtime, binds nothing.
pub fn start(host: &MobileHost) {
    *ATTACHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(host.clone());
    // Settings shows "paired" from cached facts: read the sealed identity
    // once, off the caller's thread, whether or not the link is switched on.
    let loader = host.clone();
    let _ = std::thread::Builder::new()
        .name("monocode-web-identity".into())
        .spawn(move || {
            let _ = loader.web_refresh_identity();
        });
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

/// Re-sends the current state (capabilities) on every ready web session,
/// after a policy toggle changes, so a card appears or goes away without a
/// reconnect. Does nothing when the web link is not running.
pub fn resend_state() {
    if let Some(host) = attached_host() {
        host.web_link().command(LinkCommand::ResendAll);
    }
}
