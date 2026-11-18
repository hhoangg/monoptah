//! A paired phone controls the workspace through the app that owns it. The
//! host pairs devices, serves the link and checks every request; the app
//! answers them from its own state. No arbitrary app commands are exposed.
use super::audit::{self, AuditEvent, Outcome, Transport};
use super::catalog::{recent_sessions, session_page};
use super::identity_store::{self, IdentityKeychain, SystemKeychain};
use super::policy::{self, DeviceContext, DeviceKind, Permission};
use super::web_link::{desktop_api, LinkCommand, PairPoll, PairTicket, PermissionRequest};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use monocode_mobile_link::{now, Address, Gate, Identity, WatchSink, PORT, VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

/// Methods the app answers. Pairing, leaving and the catalog are the host's.
pub(super) const APP_METHODS: &[&str] = &[
    "conversation.read",
    "conversation.older",
    "models.list",
    "accounts.usage",
    "session.configure",
    "project.changes",
    "project.worktrees",
    "project.files",
    "project.file",
    "project.search",
    "project.fileHighlight",
    "project.git",
    "project.gitAction",
    "session.start",
    "conversation.send",
    "session.resumeUsage",
    "session.resumeUsageAtReset",
    "session.answerQuestion",
    "session.keepQuestionOpen",
    "session.cancel",
    "session.rename",
    "session.delete",
    "session.archive",
    "mono.document",
    "mono.profile",
    "mono.update",
    "mono.reset",
    "mono.delete",
    "mono.create",
    "mono.file",
    "mono.saveFile",
    "session.answerApproval",
    "cloud.instances",
    "cloud.stop",
    "cloud.resume",
    "cloud.startOptions",
    "cloud.retryStart",
    "cloud.keepRunning",
    "cloud.moveToCloud",
    "cloud.moveToLocal",
    "cloud.cancelMove",
];
/// Under the phone's own wait, so it hears why rather than timing out.
const REPLY_WAIT: Duration = Duration::from_secs(25);
const MAX_PENDING: usize = 16;
const MAX_WATCHES_PER_DEVICE: usize = 4;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    /// Where the phone is notified while its app is closed, once it asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<PushRegistration>,
    /// A QUIC phone (every row an earlier version wrote) or a web device.
    #[serde(default = "phone_kind", skip_serializing_if = "is_phone_kind")]
    pub kind: DeviceKind,
    #[serde(default = "control_permission", skip_serializing_if = "is_control")]
    pub permission: Permission,
    /// The projects the device may see; `None` means all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    /// The Monos the device may see; `None` means all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monos: Option<Vec<String>>,
    /// The relay's id for a web device. Phones have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_device_id: Option<String>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
}

fn phone_kind() -> DeviceKind {
    DeviceKind::Phone
}
fn is_phone_kind(kind: &DeviceKind) -> bool {
    *kind == DeviceKind::Phone
}
fn control_permission() -> Permission {
    Permission::Control
}
fn is_control(permission: &Permission) -> bool {
    *permission == Permission::Control
}

/// The context of a request from the stored device. For a phone row an earlier
/// version wrote, this equals `DeviceContext::phone`.
fn device_context(device: &Device) -> DeviceContext {
    DeviceContext {
        id: device.id.clone(),
        kind: device.kind,
        permission: device.permission,
        projects: device.projects.clone(),
        monos: device.monos.clone(),
    }
}

/// A phone's Expo push token and the notification categories it turned off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRegistration {
    pub token: String,
    #[serde(default)]
    pub disabled: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    enabled: bool,
    #[serde(default)]
    open_router: bool,
    #[serde(default)]
    remote_address: String,
    #[serde(default)]
    devices: Vec<Device>,
    /// The web link runs independently of QUIC `enabled`. Left out of the file
    /// while off, so a phone-only file is unchanged.
    #[serde(default, skip_serializing_if = "is_false")]
    web_enabled: bool,
    /// Server device ids the user revoked or denied on this desktop. The grant
    /// sync reports each as `revoked` until the server lists it so, even when
    /// the first report was lost offline. Capped at `MAX_REVOKED_IDS` (the
    /// oldest are dropped) and cleared on Unpair and on a new pairing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    web_revoked: Vec<String>,
}
/// How many revoked server ids are remembered.
const MAX_REVOKED_IDS: usize = 256;
/// Remembers `id` as revoked by the user (no duplicates, oldest dropped).
fn note_revoked(config: &mut Config, id: &str) {
    config
        .web_revoked
        .retain(|old| !old.eq_ignore_ascii_case(id));
    config.web_revoked.push(id.to_owned());
    let extra = config.web_revoked.len().saturating_sub(MAX_REVOKED_IDS);
    config.web_revoked.drain(..extra);
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            open_router: true,
            remote_address: String::new(),
            devices: Vec::new(),
            web_enabled: false,
            web_revoked: Vec::new(),
        }
    }
}

/// What Settings shows about mobile access.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub enabled: bool,
    pub listening: bool,
    pub name: String,
    pub fingerprint: Option<String>,
    pub addresses: Vec<Address>,
    pub devices: Vec<Device>,
    pub open_router: bool,
    pub remote_address: String,
    pub router_status: String,
    /// Why the service could not start, when it could not.
    pub error: Option<String>,
}

/// A pairing code: `url` goes in the QR code and expires at `expires` (seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct Invite {
    pub url: String,
    pub expires: u64,
}

/// A phone request for the app to answer with [`Request::reply`].
pub struct Request {
    /// Scoped to the device, stable when the phone retries the same command.
    pub command_id: String,
    pub method: String,
    pub params: Value,
    /// Who asked, for the app to check what it alone can resolve.
    pub device: DeviceContext,
    reply: oneshot::Sender<Result<Value, String>>,
}
impl Request {
    pub fn reply(self, result: Result<Value, String>) {
        let _ = self.reply.send(result);
    }
}

/// A phone starts following a conversation. The app accepts or refuses it
/// with [`WatchStart::accept`], then sends frames with [`MobileHost::push`]
/// until it receives [`Event::WatchStop`] or a push reports [`Push::Gone`].
pub struct WatchStart {
    pub id: String,
    pub conversation_id: String,
    /// How many of the latest turns to open on.
    pub tail: Option<u64>,
    /// The first turn the phone already has, to carry on from instead.
    pub from: Option<String>,
    /// The phone takes field edits and text suffixes.
    pub incremental: bool,
    /// Who asked, for the app to check what it alone can resolve.
    pub device: DeviceContext,
    accept: oneshot::Sender<Result<(), String>>,
}
impl WatchStart {
    pub fn accept(self, result: Result<(), String>) {
        let _ = self.accept.send(result);
    }
}

pub enum Event {
    Request(Request),
    Watch(WatchStart),
    WatchStop(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    /// The phone has not taken earlier frames; keep the baseline and retry.
    Busy,
    /// The watch has ended.
    Gone,
}

struct Invitation {
    expires: u64,
}
struct Watcher {
    peer: String,
    /// The transport the watch runs over, so turning one transport off ends
    /// only its own watches.
    via: DeviceKind,
    sink: WatchSink,
}

/// The host's runtime. Dropping a tokio runtime from async code panics, and the
/// last clone of the host can be dropped by a task running on this very
/// runtime, so it is shut down in the background instead of joined.
struct HostRuntime(Option<tokio::runtime::Runtime>);
impl std::ops::Deref for HostRuntime {
    type Target = tokio::runtime::Runtime;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("runtime is taken only on drop")
    }
}
impl Drop for HostRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}
struct Inner {
    config: Config,
    identity: Option<Identity>,
    endpoint: Option<quinn::Endpoint>,
    invitations: HashMap<String, Invitation>,
    catalog: Value,
    watches: HashMap<String, Watcher>,
    router_address: Option<Address>,
    router_status: String,
    router_started: bool,
    error: Option<String>,
}

#[derive(Clone)]
pub struct MobileHost {
    inner: Arc<Mutex<Inner>>,
    gate: Gate,
    path: PathBuf,
    events: async_channel::Sender<Event>,
    runtime: Arc<OnceLock<HostRuntime>>,
    /// Serializes identity loading without holding `inner`, which the UI reads.
    identity_load: Arc<Mutex<()>>,
    /// Only read to migrate an identity an earlier version saved in the keychain.
    keychain: Arc<dyn IdentityKeychain>,
    pending: Arc<AtomicUsize>,
    shut_down: Arc<AtomicBool>,
    /// The web link's client task, status and pending grants.
    web: super::web_link::WebLink,
}

fn unavailable<T>(_: T) -> String {
    "Mobile service unavailable".into()
}

/// `{"token": "ExponentPushToken[…]", "disabled": [category…]}`, or a null
/// token to stop notifying the phone.
fn push_registration(params: &Value) -> Result<Option<PushRegistration>, String> {
    let token = match &params["token"] {
        Value::Null => return Ok(None),
        Value::String(token) => token,
        _ => return Err("Invalid push token".into()),
    };
    let valid = token.len() <= 256
        && token.ends_with(']')
        && (token.starts_with("ExponentPushToken[") || token.starts_with("ExpoPushToken["));
    if !valid {
        return Err("Invalid push token".into());
    }
    let disabled = params["disabled"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| value.len() <= 64)
                .take(16)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(PushRegistration {
        token: token.clone(),
        disabled,
    }))
}

impl MobileHost {
    /// Loads `path` (the desktop's `mobile-access.json`) and starts serving
    /// when mobile access is on. The app reads phone requests from the receiver.
    pub fn open(path: PathBuf) -> Result<(Self, async_channel::Receiver<Event>), String> {
        let keychain = Arc::new(SystemKeychain::new(identity_store::account_for(&path)));
        Self::open_with(path, keychain)
    }

    pub(crate) fn open_with(
        path: PathBuf,
        keychain: Arc<dyn IdentityKeychain>,
    ) -> Result<(Self, async_channel::Receiver<Event>), String> {
        let config: Config = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("Mobile settings are invalid: {e}"))?
        } else {
            Config::default()
        };
        let gate = Gate::default();
        // The QUIC gate lists phones only: a web device never connects over QUIC.
        for device in config
            .devices
            .iter()
            .filter(|device| device.kind == DeviceKind::Phone)
        {
            gate.allow(device.fingerprint.clone(), None);
        }
        let enabled = config.enabled;
        let (events, receiver) = async_channel::bounded(64);
        let host = Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                identity: None,
                endpoint: None,
                invitations: HashMap::new(),
                catalog: json!({"projects":[],"monos":[],"sessions":[]}),
                watches: HashMap::new(),
                router_address: None,
                router_status: "Off".into(),
                router_started: false,
                error: None,
            })),
            gate,
            path,
            events,
            runtime: Arc::default(),
            identity_load: Arc::default(),
            keychain,
            pending: Arc::default(),
            shut_down: Arc::default(),
            web: super::web_link::WebLink::default(),
        };
        // Loading the identity reads the database (and, once, the legacy
        // keychain entry, which can wait on an access prompt), and the app opens
        // the host while launching.
        if enabled {
            let starting = host.clone();
            std::thread::Builder::new()
                .name("monocode-mobile-start".into())
                .spawn(move || {
                    if let Err(error) = starting.start() {
                        if let Ok(mut inner) = starting.inner.lock() {
                            inner.error = Some(error);
                        }
                    }
                })
                .map_err(|e| format!("Mobile service could not start: {e}"))?;
        }
        Ok((host, receiver))
    }

    pub fn status(&self) -> Status {
        let Ok(inner) = self.inner.lock() else {
            return Status::default();
        };
        Status {
            enabled: inner.config.enabled,
            listening: inner.endpoint.is_some(),
            name: name(),
            fingerprint: inner.identity.as_ref().and_then(|v| v.fingerprint().ok()),
            addresses: addresses(&inner),
            devices: inner.config.devices.clone(),
            open_router: inner.config.open_router,
            remote_address: inner.config.remote_address.clone(),
            router_status: inner.router_status.clone(),
            error: inner.error.clone(),
        }
    }

    /// Whether the app should keep its catalog published: while phones
    /// (QUIC) or web devices can ask for it. Settings reads
    /// [`Status::enabled`] for the QUIC switch alone.
    pub fn enabled(&self) -> bool {
        self.inner
            .lock()
            .is_ok_and(|inner| inner.config.enabled || inner.config.web_enabled)
    }

    pub fn configure(
        &self,
        enabled: bool,
        open_router: bool,
        remote_address: &str,
    ) -> Result<(), String> {
        let remote_address = remote_address.trim();
        if !remote_address.is_empty() {
            Address::parse(remote_address)?;
        }
        {
            let mut inner = self.inner.lock().map_err(unavailable)?;
            let config = Config {
                enabled,
                open_router,
                remote_address: remote_address.into(),
                devices: inner.config.devices.clone(),
                web_enabled: inner.config.web_enabled,
                web_revoked: inner.config.web_revoked.clone(),
            };
            self.save(&config)?;
            inner.config = config;
            inner.error = None;
            if !enabled {
                if let Some(endpoint) = inner.endpoint.take() {
                    endpoint.close(1_u32.into(), b"Mobile access disabled");
                }
                for key in inner.invitations.keys() {
                    self.gate.remove(key);
                }
                inner.invitations.clear();
                // Web devices keep their watches: their transport is separate.
                inner
                    .watches
                    .retain(|_, watch| watch.via != DeviceKind::Phone);
            }
        }
        if enabled {
            if let Err(error) = self.start() {
                self.inner.lock().map_err(unavailable)?.error = Some(error.clone());
                return Err(error);
            }
        }
        super::router::restart(self);
        Ok(())
    }

    /// One visible invitation at a time; a new one revokes the last.
    pub fn invite(&self) -> Result<Invite, String> {
        let identity = self.identity()?;
        let mut inner = self.inner.lock().map_err(unavailable)?;
        if !inner.config.enabled || inner.endpoint.is_none() {
            return Err("Enable mobile access first".into());
        }
        for key in inner.invitations.keys() {
            self.gate.remove(key);
        }
        inner.invitations.clear();
        let join = Identity::generate()?;
        let key = join.fingerprint()?;
        let expires = now() + 15 * 60;
        self.gate.allow(key.clone(), Some(expires));
        inner.invitations.insert(key, Invitation { expires });
        let invitation = json!({"version":VERSION,"name":name(),"fingerprint":identity.fingerprint()?,"addresses":addresses(&inner),"identity":join,"expires":expires});
        let code =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invitation).map_err(|e| e.to_string())?);
        Ok(Invite {
            url: format!("monocode://pair?i={code}"),
            expires,
        })
    }

    pub fn cancel_invite(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            for key in inner.invitations.keys() {
                self.gate.remove(key);
            }
            inner.invitations.clear();
        }
    }

    pub fn revoke(&self, device_id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(unavailable)?;
        let mut config = inner.config.clone();
        let removed = config
            .devices
            .iter()
            .find(|v| v.id == device_id)
            .map(|device| (device.kind, device.fingerprint.clone()));
        if let Some((kind, peer)) = &removed {
            if *kind == DeviceKind::Phone {
                self.gate.remove(peer);
            }
            close_device_watches(&mut inner, peer);
        }
        // Only the row of the kind found goes: ids of different kinds are
        // unrelated.
        if let Some((kind, _)) = removed {
            config
                .devices
                .retain(|device| !(device.id == device_id && device.kind == kind));
        }
        self.save(&config)?;
        inner.config = config;
        Ok(())
    }

    /// The app's workspace: `projects`, `monos` and `sessions` (newest first).
    pub fn publish(&self, catalog: Value) -> Result<(), String> {
        if !catalog["projects"].is_array()
            || !catalog["monos"].is_array()
            || !catalog["sessions"].is_array()
        {
            return Err("Invalid mobile catalog".into());
        }
        self.inner.lock().map_err(unavailable)?.catalog = catalog;
        Ok(())
    }

    /// The push tokens of paired phones that take `category`.
    pub fn push_tokens(&self, category: &str) -> Vec<String> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        if !inner.config.enabled {
            return Vec::new();
        }
        inner
            .config
            .devices
            .iter()
            .filter(|device| device.kind == DeviceKind::Phone)
            .filter_map(|device| device.push.as_ref())
            .filter(|push| !push.disabled.iter().any(|off| off == category))
            .map(|push| push.token.clone())
            .collect()
    }

    /// Drops tokens Expo says no longer reach a phone, as when the app was
    /// removed; the phone registers again the next time it connects.
    pub fn forget_push_tokens(&self, tokens: &[String]) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(unavailable)?;
        let mut config = inner.config.clone();
        let mut changed = false;
        for device in &mut config.devices {
            if device
                .push
                .as_ref()
                .is_some_and(|push| tokens.contains(&push.token))
            {
                device.push = None;
                changed = true;
            }
        }
        if changed {
            self.save(&config)?;
            inner.config = config;
        }
        Ok(())
    }

    /// The watch's next frame.
    pub fn push(&self, id: &str, update: Value) -> Result<(), Push> {
        let mut inner = self.inner.lock().map_err(|_| Push::Gone)?;
        let watch = inner.watches.get(id).ok_or(Push::Gone)?;
        match watch.sink.try_send(update) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Err(Push::Busy),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                inner.watches.remove(id);
                Err(Push::Gone)
            }
        }
    }

    /// Stops serving and releases the router mapping; for app shutdown.
    pub fn shutdown(&self) {
        self.shut_down.store(true, Ordering::SeqCst);
        self.web.stop();
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(endpoint) = inner.endpoint.take() {
                endpoint.close(0_u32.into(), b"Desktop closed");
            }
            inner.watches.clear();
        }
    }

    pub(super) fn runtime(&self) -> Result<&tokio::runtime::Runtime, String> {
        if let Some(runtime) = self.runtime.get() {
            return Ok(&**runtime);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("monocode-mobile")
            .enable_all()
            .build()
            .map_err(|e| format!("Mobile service could not start: {e}"))?;
        Ok(&**self.runtime.get_or_init(|| HostRuntime(Some(runtime))))
    }

    fn start(&self) -> Result<(), String> {
        let identity = self.identity()?;
        let runtime = self.runtime()?;
        let mut inner = self.inner.lock().map_err(unavailable)?;
        if !inner.config.enabled
            || self.shut_down.load(Ordering::SeqCst)
            || inner.endpoint.is_some()
        {
            return Ok(());
        }
        let endpoint = {
            let _context = runtime.enter();
            monocode_mobile_link::server_endpoint(
                &identity,
                self.gate.clone(),
                format!("[::]:{PORT}").parse().unwrap(),
            )
            .map_err(|error| {
                // Another copy of the app (or the earlier desktop) holds the
                // port; phones would reach it instead of this one.
                if error.contains("in use") || error.contains("os error 48") {
                    format!("Another Monoptah window or app is already accepting phone connections on port {PORT}. Quit it, then turn Accept connections off and on.")
                } else {
                    error
                }
            })?
        };
        inner.endpoint = Some(endpoint.clone());
        drop(inner);
        let service = self.clone();
        let watching = self.clone();
        runtime.spawn(monocode_mobile_link::serve(
            endpoint,
            self.gate.clone(),
            move |peer, request| {
                let host = service.clone();
                async move { host.handle(&peer, request).await }
            },
            move |peer, request, sink| {
                let host = watching.clone();
                async move { host.watch(&peer, request, sink).await }
            },
        ));
        super::router::restart(self);
        Ok(())
    }

    fn save(&self, config: &Config) -> Result<(), String> {
        let parent = self.path.parent().ok_or("Invalid mobile settings path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temporary = parent.join(format!(".mobile-access-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(
            &temporary,
            serde_json::to_vec(config).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &self.path).map_err(|e| e.to_string())
    }

    /// The desktop's long-lived identity, sealed in the native store database.
    /// An identity saved in the OS keychain by an earlier version is migrated
    /// once, so paired phones stay paired.
    fn identity(&self) -> Result<Identity, String> {
        let _loading = self.identity_load.lock().map_err(unavailable)?;
        let paired = {
            let inner = self.inner.lock().map_err(unavailable)?;
            if let Some(identity) = &inner.identity {
                return Ok(identity.clone());
            }
            !inner.config.devices.is_empty()
        };
        let database = self.path.parent().and_then(crate::accounts::database_for);
        let identity = identity_store::load(
            database.as_deref(),
            &identity_store::account_for(&self.path),
            paired,
            self.keychain.as_ref(),
        )?;
        self.inner.lock().map_err(unavailable)?.identity = Some(identity.clone());
        Ok(identity)
    }

    fn enroll(&self, inner: &mut Inner, peer: &str, params: &Value) -> Result<Value, String> {
        let invitation = inner
            .invitations
            .get(peer)
            .ok_or("Invitation already used or revoked")?;
        if invitation.expires <= now() {
            return Err("Invitation expired".into());
        }
        let certificate = params["certificate"]
            .as_str()
            .ok_or("Missing device identity")?;
        let proof = params["proof"].as_str().ok_or("Missing device proof")?;
        let fingerprint = monocode_mobile_link::verify_proof(certificate, peer, proof)?;
        let name = params["name"]
            .as_str()
            .unwrap_or("Phone")
            .trim()
            .chars()
            .take(80)
            .collect::<String>();
        let id = uuid::Uuid::new_v4().to_string();
        let mut config = inner.config.clone();
        config
            .devices
            .retain(|device| device.fingerprint != fingerprint);
        config.devices.push(Device {
            id: id.clone(),
            name,
            fingerprint: fingerprint.clone(),
            push: None,
            kind: DeviceKind::Phone,
            permission: Permission::Control,
            projects: None,
            monos: None,
            server_device_id: None,
            created_at: None,
            last_seen: None,
        });
        self.save(&config)?;
        inner.config = config;
        inner.invitations.remove(peer);
        self.gate.remove(peer);
        self.gate.allow(fingerprint, None);
        let desktop = inner
            .identity
            .as_ref()
            .ok_or("Desktop mobile identity is unavailable")?
            .fingerprint()?;
        Ok(
            json!({"deviceId":id,"name":self::name(),"fingerprint":desktop,"addresses":addresses(inner)}),
        )
    }

    /// A QUIC phone's request.
    pub(super) async fn handle(&self, peer: &str, request: Value) -> Result<Value, String> {
        self.handle_from(DeviceKind::Phone, peer, request).await
    }

    /// A web device's request, delivered by the web link. Needs `web_enabled`
    /// and a web row; a phone row is never served here.
    pub(super) async fn handle_web(&self, peer: &str, request: Value) -> Result<Value, String> {
        self.handle_from(DeviceKind::Web, peer, request).await
    }

    async fn handle_from(
        &self,
        via: DeviceKind,
        peer: &str,
        request: Value,
    ) -> Result<Value, String> {
        let mut audited = None;
        let result = self.call_from(via, peer, &request, &mut audited).await;
        // Recorded with no lock held: the log may write a file.
        if let Some((context, mut event)) = audited {
            event.outcome = final_outcome(event.outcome, result.is_err());
            audit::record(&context, &event);
        }
        result
    }

    /// `handle` without the audit record: it leaves the event of an
    /// identified device in `audited` for `handle` to record once the call
    /// has an outcome.
    #[cfg(test)] // QUIC phone call, kept for the audit tests
    async fn handle_call(
        &self,
        peer: &str,
        request: &Value,
        audited: &mut Option<(DeviceContext, AuditEvent)>,
    ) -> Result<Value, String> {
        self.call_from(DeviceKind::Phone, peer, request, audited)
            .await
    }

    /// `handle_call` for the transport `via`.
    async fn call_from(
        &self,
        via: DeviceKind,
        peer: &str,
        request: &Value,
        audited: &mut Option<(DeviceContext, AuditEvent)>,
    ) -> Result<Value, String> {
        if request["version"] != VERSION {
            return Err("Update Monoptah on both devices".into());
        }
        let method = request["method"].as_str().ok_or("Invalid mobile method")?;
        let params = request["params"].clone();
        if !params.is_object() {
            return Err("Invalid mobile parameters".into());
        }
        let device = {
            let mut inner = self.inner.lock().map_err(unavailable)?;
            check_enabled(&inner, via)?;
            // Pairing is the QUIC phones' own flow. The web transport pairs
            // through the relay, so a web context must never enrol here.
            if method == "device.enroll" {
                if !enroll_allowed(via) {
                    return Err("Unsupported mobile operation".into());
                }
                return self.enroll(&mut inner, peer, &params);
            }
            let device = find_device(&inner, via, peer)?;
            let context = device_context(&device);
            *audited = Some((context.clone(), audit_event(&device, method, request)));
            if let Err(denial) = policy::authorize(&context, method, &params) {
                mark_denied(audited, &denial);
                return Err(denial.error);
            }
            match method {
                "device.selection" => {
                    let answer = policy::device_selection(&context, &params)?;
                    return self.answer_selection(&mut inner, peer, answer);
                }
                "device.leave" => {
                    let mut config = inner.config.clone();
                    config
                        .devices
                        .retain(|v| !(v.id == device.id && v.kind == device.kind));
                    self.save(&config)?;
                    inner.config = config;
                    if via == DeviceKind::Phone {
                        self.gate.remove(peer);
                    }
                    close_device_watches(&mut inner, peer);
                    return Ok(Value::Null);
                }
                "workspace.list" => {
                    let mut catalog = inner.catalog.clone();
                    catalog["sessions"] = json!(recent_sessions(&catalog["sessions"]));
                    // The desktop's network addresses are for phones; a browser
                    // reaches it through the relay and never needs them.
                    if context.kind != DeviceKind::Web {
                        catalog["addresses"] = json!(addresses(&inner));
                    }
                    policy::filter_reply(&context, method, &mut catalog);
                    return Ok(catalog);
                }
                "sessions.list" => {
                    let mut page = session_page(&inner.catalog["sessions"], &params);
                    policy::filter_reply(&context, method, &mut page);
                    return Ok(page);
                }
                // Push tokens are for the phones' Expo path; the web transport
                // answers `unsupported` (the code is added on the web channel).
                "device.push" if via == DeviceKind::Web => {
                    return Err("Unsupported mobile operation".into());
                }
                "device.push" => {
                    let push = push_registration(&params)?;
                    let mut config = inner.config.clone();
                    if let Some(entry) = config.devices.iter_mut().find(|v| v.id == device.id) {
                        entry.push = push;
                    }
                    self.save(&config)?;
                    inner.config = config;
                    return Ok(Value::Null);
                }
                _ => {}
            }
            if !APP_METHODS.contains(&method) {
                return Err("Unsupported mobile operation".into());
            }
            context
        };
        let command_id = request["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or("Missing command ID")?;
        if self.pending.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING {
            self.pending.fetch_sub(1, Ordering::SeqCst);
            return Err("Desktop is busy; try again".into());
        }
        let (reply, receive) = oneshot::channel();
        let sent = self
            .events
            .try_send(Event::Request(Request {
                command_id: format!("{}:{command_id}", device.id),
                method: method.into(),
                params,
                device,
                reply,
            }))
            .is_ok();
        let response = if sent {
            tokio::time::timeout(REPLY_WAIT, receive)
                .await
                .map_err(|_| "Desktop did not answer".to_string())
                .and_then(|reply| reply.map_err(|_| "Desktop is unavailable".to_string()))
        } else {
            Err("Desktop is unavailable".to_string())
        };
        self.pending.fetch_sub(1, Ordering::SeqCst);
        let response = response?;
        let admitted = match via {
            DeviceKind::Phone => self.gate.admits(peer),
            DeviceKind::Web => self
                .inner
                .lock()
                .is_ok_and(|inner| find_device(&inner, via, peer).is_ok()),
        };
        if !admitted {
            return Err("Device revoked".into());
        }
        response
    }

    /// A phone starts watching a conversation. The app sends its state, then
    /// every change, until the phone goes or the watch is stopped.
    pub(super) async fn watch(
        &self,
        peer: &str,
        request: Value,
        sink: WatchSink,
    ) -> Result<(), String> {
        self.watch_from(DeviceKind::Phone, peer, request, sink)
            .await
    }

    /// `watch` for a web device, as `handle_web` is to `handle`.
    pub(super) async fn watch_web(
        &self,
        peer: &str,
        request: Value,
        sink: WatchSink,
    ) -> Result<(), String> {
        self.watch_from(DeviceKind::Web, peer, request, sink).await
    }

    async fn watch_from(
        &self,
        via: DeviceKind,
        peer: &str,
        request: Value,
        sink: WatchSink,
    ) -> Result<(), String> {
        let mut audited = None;
        let result = self
            .watch_call_from(via, peer, &request, sink, &mut audited)
            .await;
        // Recorded with no lock held: the log may write a file.
        if let Some((context, mut event)) = audited {
            event.outcome = final_outcome(event.outcome, result.is_err());
            audit::record(&context, &event);
        }
        result
    }

    /// `watch` without the audit record, as `handle_call` is to `handle`.
    async fn watch_call_from(
        &self,
        via: DeviceKind,
        peer: &str,
        request: &Value,
        sink: WatchSink,
        audited: &mut Option<(DeviceContext, AuditEvent)>,
    ) -> Result<(), String> {
        if request["version"] != VERSION {
            return Err("Update Monoptah on both devices".into());
        }
        if request["method"] != "conversation.watch" {
            return Err("Unsupported mobile operation".into());
        }
        let params = &request["params"];
        let conversation = params["conversationId"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 256)
            .ok_or("Missing workspace item")?
            .to_string();
        let id = uuid::Uuid::new_v4().to_string();
        let context = {
            let mut inner = self.inner.lock().map_err(unavailable)?;
            check_enabled(&inner, via)?;
            let device = find_device(&inner, via, peer)?;
            let context = device_context(&device);
            *audited = Some((
                context.clone(),
                audit_event(&device, "conversation.watch", request),
            ));
            if let Err(denial) = policy::authorize(&context, "conversation.watch", params) {
                mark_denied(audited, &denial);
                return Err(denial.error);
            }
            inner.watches.retain(|_, watch| !watch.sink.is_closed());
            if inner
                .watches
                .values()
                .filter(|watch| watch.peer == peer)
                .count()
                >= MAX_WATCHES_PER_DEVICE
            {
                return Err("Desktop is busy; try again".into());
            }
            inner.watches.insert(
                id.clone(),
                Watcher {
                    peer: peer.into(),
                    via,
                    sink: sink.clone(),
                },
            );
            context
        };
        let (accept, accepted) = oneshot::channel();
        let start = Event::Watch(WatchStart {
            id: id.clone(),
            conversation_id: conversation,
            tail: params["tail"].as_u64(),
            from: params["from"]
                .as_str()
                .filter(|from| from.len() <= 256)
                .map(Into::into),
            incremental: params["incremental"] == true,
            device: context,
            accept,
        });
        let accepted = if self.events.try_send(start).is_ok() {
            tokio::time::timeout(REPLY_WAIT, accepted)
                .await
                .map_err(|_| "Desktop did not answer".to_string())
                .and_then(|result| result.map_err(|_| "Desktop is unavailable".to_string()))
                .and_then(|result| result)
        } else {
            Err("Desktop is unavailable".into())
        };
        if let Err(error) = accepted {
            if let Ok(mut inner) = self.inner.lock() {
                inner.watches.remove(&id);
            }
            return Err(error);
        }
        // End the watch with the phone or the host, whichever goes first.
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = sink.closed() => break,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        if !host.inner.lock().is_ok_and(|inner| inner.watches.contains_key(&id)) {
                            break;
                        }
                    }
                }
            }
            if let Ok(mut inner) = host.inner.lock() {
                inner.watches.remove(&id);
            }
            let _ = host.events.send(Event::WatchStop(id)).await;
        });
        Ok(())
    }

    pub(super) fn router_config(&self) -> bool {
        self.inner.lock().is_ok_and(|inner| {
            inner.config.enabled && inner.config.open_router && inner.endpoint.is_some()
        })
    }
    pub(super) fn router_result(&self, address: Option<Address>, status: String) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.router_address = address;
            inner.router_status = status;
        }
    }
    pub(super) fn router_claim(&self) -> bool {
        self.inner
            .lock()
            .is_ok_and(|mut inner| !std::mem::replace(&mut inner.router_started, true))
    }
    pub(super) fn is_shut_down(&self) -> bool {
        self.shut_down.load(Ordering::SeqCst)
    }
}

/// Whether the transport `via` is switched on: QUIC phones need `enabled`, web
/// devices need `web_enabled`, and neither depends on the other.
fn check_enabled(inner: &Inner, via: DeviceKind) -> Result<(), String> {
    match via {
        DeviceKind::Phone if !inner.config.enabled => Err("Mobile access disabled".into()),
        DeviceKind::Web if !inner.config.web_enabled => Err("Web access disabled".into()),
        _ => Ok(()),
    }
}

/// The paired device with this fingerprint that belongs to the transport: a
/// QUIC phone is never found through the web path, nor a web device through QUIC.
fn find_device(inner: &Inner, via: DeviceKind, peer: &str) -> Result<Device, String> {
    inner
        .config
        .devices
        .iter()
        .find(|device| device.fingerprint == peer && device.kind == via)
        .cloned()
        .ok_or_else(|| "Device revoked or not paired".to_string())
}

/// Ends every open watch of the device behind `peer`, as its selection,
/// permission or pairing changed. The phone reopens, and the app checks the
/// new access again.
fn close_device_watches(inner: &mut Inner, peer: &str) {
    inner.watches.retain(|_, watch| watch.peer != peer);
}

/// What the audit log records of a call: who, how and which method, never the
/// parameters.
fn audit_event(device: &Device, method: &str, request: &Value) -> AuditEvent {
    AuditEvent {
        fingerprint_prefix: device.fingerprint.chars().take(8).collect(),
        transport: match device.kind {
            DeviceKind::Phone => Transport::Quic,
            DeviceKind::Web => Transport::Web,
        },
        method: method.into(),
        project_id: request["params"]["projectId"].as_str().map(Into::into),
        outcome: Outcome::Ok,
        command_id: request["id"].as_str().map(Into::into),
    }
}

impl MobileHost {
    /// Answers `device.selection`. A narrower selection is written into the
    /// device's row first; the device's watches end only once it is saved. A
    /// save failure leaves the config and the watches as they were.
    fn answer_selection(
        &self,
        inner: &mut Inner,
        peer: &str,
        answer: policy::SelectionAnswer,
    ) -> Result<Value, String> {
        if let Some(selection) = answer.narrowed {
            let mut config = inner.config.clone();
            let device = config
                .devices
                .iter_mut()
                .find(|device| device.fingerprint == peer)
                .ok_or("Device revoked or not paired")?;
            device.projects = selection.projects;
            device.monos = selection.monos;
            self.save(&config)?;
            inner.config = config;
            close_device_watches(inner, peer);
        }
        Ok(answer.reply)
    }

    fn web_database(&self) -> Result<PathBuf, String> {
        self.path
            .parent()
            .and_then(crate::accounts::database_for)
            .ok_or_else(|| identity_store::UNAVAILABLE.to_owned())
    }

    /// The web link's sealed identity. `Ok(None)` means none was saved.
    pub(crate) fn load_web_identity(&self) -> Result<Option<identity_store::WebIdentity>, String> {
        let identity = identity_store::load_web(
            &self.web_database()?,
            &identity_store::web_account_for(&self.path),
        )?;
        match &identity {
            Some(identity) => self.web.remember(identity),
            None => self.web.forget_facts(),
        }
        Ok(identity)
    }

    /// Seals the web identity. Its token never goes to `mobile-access.json`.
    pub(crate) fn save_web_identity(
        &self,
        identity: &identity_store::WebIdentity,
    ) -> Result<(), String> {
        identity_store::save_web(
            &self.web_database()?,
            &identity_store::web_account_for(&self.path),
            identity,
        )?;
        self.web.remember(identity);
        Ok(())
    }

    /// Reads the sealed web identity once into the facts `web_status` shows.
    /// Blocking (a database read): call it off the UI thread. The app does so
    /// at start through `web_link::start`.
    pub fn web_refresh_identity(&self) -> Result<(), String> {
        self.load_web_identity().map(|_| ())
    }
}

/// The server-side id of the grant behind a row, as reported back.
type Report = (String, &'static str, Option<Permission>);

impl MobileHost {
    /// Starts a pairing with the web server at `host_url`: shows a user code
    /// and a link whose `#fp=` fragment carries this desktop's key fingerprint.
    /// Blocking (an HTTP call). Refused while a web identity exists.
    pub fn web_pair_start(&self, host_url: &str, machine_name: &str) -> Result<PairTicket, String> {
        self.web_pair_start_with(host_url, machine_name, cfg!(debug_assertions))
    }

    pub(super) fn web_pair_start_with(
        &self,
        host_url: &str,
        machine_name: &str,
        allow_insecure_localhost: bool,
    ) -> Result<PairTicket, String> {
        use rand::RngCore;
        use sha2::{Digest, Sha256};
        let base = desktop_api::base_url(host_url, allow_insecure_localhost)?;
        if self.web.is_paired() {
            return Err("Unpair first".into());
        }
        match self.load_web_identity() {
            Ok(Some(_)) => return Err("Unpair first".into()),
            Ok(None) => {}
            // A damaged identity counts as a reinstall: pairing replaces it.
            Err(error) if error == identity_store::WEB_UNREADABLE => {}
            Err(error) => return Err(error),
        }
        let (private, public) = super::web_link::noise::generate_static()?;
        let private = Zeroizing::new(private);
        let mut verifier = Zeroizing::new([0u8; 32]);
        rand::rng().fill_bytes(&mut *verifier);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(*verifier));
        let machine_name: String = machine_name.trim().chars().take(64).collect();
        let machine_name = if machine_name.is_empty() {
            "Desktop".to_owned()
        } else {
            machine_name
        };
        let started = desktop_api::pair_start(
            &base,
            &URL_SAFE_NO_PAD.encode(public),
            &machine_name,
            &challenge,
        )?;
        let code_ok = !started.user_code.is_empty()
            && started
                .user_code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
        // The confirmation code is shown to the user: plain short text only.
        let confirmation_ok = !started.confirmation_code.is_empty()
            && started.confirmation_code.len() <= 16
            && started
                .confirmation_code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !code_ok || !confirmation_ok {
            return Err("The web server sent an invalid pairing code".into());
        }
        let fingerprint = super::web_link::noise::fingerprint(&public);
        let ticket = PairTicket {
            link: format!("{base}/pair/{}#fp={fingerprint}", started.user_code),
            user_code: started.user_code,
            confirmation_code: started.confirmation_code,
            fingerprint,
            expires_at: now().saturating_add(started.expires_in),
            interval: started.interval,
        };
        *self.web.pairing() = Some(super::web_link::Pairing {
            id: self.web.next_pairing_id(),
            device_code: started.device_code,
            verifier,
            private,
            public,
            base,
            not_before: None,
        });
        Ok(ticket)
    }

    /// Asks the web server whether the pairing was approved. Blocking; call
    /// it every `PairTicket::interval` seconds. On approval the identity is
    /// sealed, the web link is switched on and started.
    pub fn web_pair_poll(&self) -> Result<PairPoll, String> {
        use super::web_link::desktop_api::PollOutcome;
        let (id, base, device_code, verifier) = {
            let guard = self.web.pairing();
            let pairing = guard.as_ref().ok_or("No pairing in progress")?;
            if pairing
                .not_before
                .is_some_and(|until| std::time::Instant::now() < until)
            {
                return Ok(PairPoll::Pending);
            }
            (
                pairing.id,
                pairing.base.clone(),
                pairing.device_code.clone(),
                Zeroizing::new(URL_SAFE_NO_PAD.encode(*pairing.verifier)),
            )
        };
        let outcome = desktop_api::pair_poll(&base, &device_code, &verifier)?;
        let still_current = |pairing: &Option<super::web_link::Pairing>| {
            pairing.as_ref().is_some_and(|p| p.id == id)
        };
        match outcome {
            PollOutcome::Pending => Ok(PairPoll::Pending),
            PollOutcome::SlowDown { retry_after } => {
                if let Some(pairing) = self.web.pairing().as_mut().filter(|p| p.id == id) {
                    let wait = retry_after.unwrap_or(3).clamp(1, 60);
                    pairing.not_before =
                        Some(std::time::Instant::now() + Duration::from_secs(wait));
                }
                Ok(PairPoll::Pending)
            }
            PollOutcome::Denied | PollOutcome::Expired => {
                let mut guard = self.web.pairing();
                if still_current(&guard) {
                    *guard = None;
                }
                Ok(if matches!(outcome, PollOutcome::Denied) {
                    PairPoll::Denied
                } else {
                    PairPoll::Expired
                })
            }
            PollOutcome::Approved(approval) => self.finish_pairing(id, &base, approval),
        }
    }

    fn finish_pairing(
        &self,
        id: u64,
        typed_base: &str,
        approval: super::web_link::desktop_api::Approval,
    ) -> Result<PairPoll, String> {
        let dev = cfg!(debug_assertions);
        let pairing = {
            let mut guard = self.web.pairing();
            if guard.as_ref().is_some_and(|p| p.id == id) {
                guard.take()
            } else {
                None
            }
        };
        // The server made a desktop for this pairing. If it cannot be used
        // (cancelled, or the answer names another host), give it back to the
        // host the user typed instead of leaving it behind.
        let undo = |base: &str| {
            let _ = desktop_api::delete_me(base, &approval.desktop_token, Duration::from_secs(8));
        };
        let Some(pairing) = pairing else {
            // Cancelled while the answer was on its way: give the desktop back
            // to the host the user typed, never to the one the answer names.
            undo(typed_base);
            return Err("The pairing was cancelled".into());
        };
        // The relay must be on the host the user typed, and its path id must be
        // this desktop's id: the server cannot move the token elsewhere.
        let relay_base = pairing.base.clone();
        let same_host = desktop_api::base_url(&approval.relay_url, dev)
            .is_ok_and(|base| base.eq_ignore_ascii_case(&relay_base));
        let same_id = desktop_api::relay_path_id(&approval.relay_url)
            .is_none_or(|id| id.eq_ignore_ascii_case(&approval.desktop_id));
        let valid_id = uuid::Uuid::parse_str(&approval.desktop_id).is_ok();
        if !(same_host && same_id && valid_id) {
            undo(&relay_base);
            return Err("The web server sent an invalid pairing".into());
        }
        // A new pairing starts every device over: old web rows would skip the
        // re-confirmation a replaced pairing needs.
        if let Err(error) = self.remove_web_rows() {
            undo(&relay_base);
            return Err(error);
        }
        let handle = super::web_link::session::clean_name(&approval.handle);
        let identity = identity_store::WebIdentity {
            static_private: *pairing.private,
            static_public: pairing.public,
            desktop_id: approval.desktop_id.clone(),
            desktop_token: (*approval.desktop_token).clone(),
            relay_url: relay_base.clone(),
            account_handle: Some(handle.clone()),
        };
        if let Err(error) = self.save_web_identity(&identity) {
            undo(&relay_base);
            return Err(error);
        }
        self.set_web_enabled(true)?;
        Ok(PairPoll::Approved { account: handle })
    }

    /// Drops a pairing in progress. Nothing is sent: the server forgets it.
    pub fn web_pair_cancel(&self) {
        *self.web.pairing() = None;
    }

    /// Removes every web device row and its watches.
    fn remove_web_rows(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(unavailable)?;
        self.remove_web_rows_locked(&mut inner)
    }

    fn remove_web_rows_locked(&self, inner: &mut Inner) -> Result<(), String> {
        let mut config = inner.config.clone();
        config
            .devices
            .retain(|device| device.kind != DeviceKind::Web);
        config.web_revoked.clear();
        if config.devices.len() != inner.config.devices.len()
            || !inner.config.web_revoked.is_empty()
        {
            self.save(&config)?;
            inner.config = config;
        }
        inner
            .watches
            .retain(|_, watch| watch.via != DeviceKind::Web);
        Ok(())
    }

    /// Reports a grant state to the server on a background thread, so the
    /// desktop's own action works while the server is offline. A report that
    /// fails is repeated by the next grant sync, which compares the server's
    /// state with the rows (`grants::plan`, `reconfirm`).
    fn report_async(
        &self,
        server_device_id: Option<String>,
        state: &'static str,
        permission: Option<Permission>,
    ) {
        let Some(server_device_id) = server_device_id else {
            return;
        };
        let host = self.clone();
        let _ = std::thread::Builder::new()
            .name("monocode-web-report".into())
            .spawn(move || {
                // Read the store directly: this must not bring back the cached
                // facts of an identity that Unpair wipes meanwhile.
                let Ok(database) = host.web_database() else {
                    return;
                };
                let Ok(Some(identity)) = identity_store::load_web(
                    &database,
                    &identity_store::web_account_for(&host.path),
                ) else {
                    return;
                };
                let Ok(base) = desktop_api::base_url(&identity.relay_url, cfg!(debug_assertions))
                else {
                    return;
                };
                let _ = desktop_api::report_grant_state(
                    &base,
                    &identity.desktop_token,
                    &server_device_id,
                    state,
                    permission,
                );
            });
    }

    /// Confirms the pending device with this fingerprint: writes its web row,
    /// moves its live session to ready and tells the server.
    pub fn web_confirm(
        &self,
        fingerprint: &str,
        permission: Permission,
        projects: Option<Vec<String>>,
        monos: Option<Vec<String>>,
    ) -> Result<Device, String> {
        let device = {
            let mut inner = self.inner.lock().map_err(unavailable)?;
            let grant = self
                .web
                .pending()
                .get(fingerprint)
                .ok_or("Unknown device: it is not waiting for confirmation")?;
            if inner
                .config
                .devices
                .iter()
                .any(|device| device.fingerprint == fingerprint)
            {
                return Err("This device is already paired".into());
            }
            // One relay device id belongs to one row.
            let taken = inner.config.devices.iter().any(|device| {
                device.kind == DeviceKind::Web
                    && device
                        .server_device_id
                        .as_deref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(&grant.server_device_id))
            });
            if taken {
                return Err("Another paired device already uses this web device id".into());
            }
            let device = Device {
                id: uuid::Uuid::new_v4().to_string(),
                name: super::web_link::session::clean_name(&grant.name),
                fingerprint: fingerprint.to_owned(),
                push: None,
                kind: DeviceKind::Web,
                permission,
                projects,
                monos,
                server_device_id: Some(grant.server_device_id),
                created_at: Some(now()),
                last_seen: None,
            };
            let mut config = inner.config.clone();
            config.devices.push(device.clone());
            self.save(&config)?;
            inner.config = config;
            self.web.pending().remove(fingerprint);
            device
        };
        self.web.command(LinkCommand::Confirm {
            fingerprint: fingerprint.to_owned(),
            device: Box::new(device.clone()),
        });
        self.report_async(
            device.server_device_id.clone(),
            "confirmed",
            Some(permission),
        );
        Ok(device)
    }

    /// Refuses the pending device: its live session ends and the server hears
    /// `revoked`.
    pub fn web_deny(&self, fingerprint: &str) -> Result<(), String> {
        let grant = self
            .web
            .pending()
            .get(fingerprint)
            .ok_or("Unknown device: it is not waiting for confirmation")?;
        {
            // Durable first: a lost report is repeated by the grant sync.
            let mut inner = self.inner.lock().map_err(unavailable)?;
            let mut config = inner.config.clone();
            note_revoked(&mut config, &grant.server_device_id);
            self.save(&config)?;
            inner.config = config;
            self.web.pending().remove(fingerprint);
        }
        self.web.command(LinkCommand::Deny {
            fingerprint: fingerprint.to_owned(),
        });
        self.report_async(Some(grant.server_device_id), "revoked", None);
        Ok(())
    }

    /// Removes a paired web device: its watches and live sessions end, and the
    /// server hears `revoked` (best effort, so it works offline).
    pub fn web_revoke(&self, device_id: &str) -> Result<(), String> {
        let removed = {
            let mut inner = self.inner.lock().map_err(unavailable)?;
            let mut config = inner.config.clone();
            let position = config
                .devices
                .iter()
                .position(|device| device.id == device_id && device.kind == DeviceKind::Web)
                .ok_or("Unknown web device")?;
            let removed = config.devices.remove(position);
            if let Some(id) = &removed.server_device_id {
                note_revoked(&mut config, id);
            }
            self.save(&config)?;
            inner.config = config;
            close_device_watches(&mut inner, &removed.fingerprint);
            removed
        };
        self.web.drop_request(device_id);
        self.web.command(LinkCommand::Revoke {
            fingerprint: removed.fingerprint,
        });
        self.report_async(removed.server_device_id, "revoked", None);
        Ok(())
    }

    /// Sets what a paired device may see and do, for a web device or a phone
    /// ("Your phones": the project selection). Its watches end so the next
    /// request is checked against the new access; a web device also gets a
    /// fresh `state` and the server hears the permission.
    pub fn set_device_access(
        &self,
        device_id: &str,
        permission: Permission,
        projects: Option<Vec<String>>,
        monos: Option<Vec<String>>,
    ) -> Result<Device, String> {
        let device = {
            let mut inner = self.inner.lock().map_err(unavailable)?;
            let mut config = inner.config.clone();
            let device = config
                .devices
                .iter_mut()
                .find(|device| device.id == device_id)
                .ok_or("Device revoked or not paired")?;
            device.permission = permission;
            device.projects = projects;
            device.monos = monos;
            let device = device.clone();
            self.save(&config)?;
            inner.config = config;
            close_device_watches(&mut inner, &device.fingerprint);
            device
        };
        if device.kind == DeviceKind::Web {
            self.web.drop_request(&device.id);
            self.web.command(LinkCommand::Resend {
                fingerprint: device.fingerprint.clone(),
            });
            self.report_async(
                device.server_device_id.clone(),
                "confirmed",
                Some(permission),
            );
        }
        Ok(device)
    }

    /// Answers a confirmed device's request for another permission. Approving
    /// applies it; denying keeps the permission and hides the request until
    /// the server's `requestedPermission` changes.
    pub fn web_answer_request(&self, device_id: &str, approve: bool) -> Result<(), String> {
        let request = self
            .web
            .request_for(device_id)
            .ok_or("There is no such permission request")?;
        if !approve {
            self.web.hide_request(&request);
            return Ok(());
        }
        let (projects, monos) = {
            let inner = self.inner.lock().map_err(unavailable)?;
            let device = inner
                .config
                .devices
                .iter()
                .find(|device| device.id == device_id && device.kind == DeviceKind::Web)
                .ok_or("Unknown web device")?;
            (device.projects.clone(), device.monos.clone())
        };
        self.set_device_access(device_id, request.requested, projects, monos)
            .map(|_| ())
    }

    /// Unpairs the web link: tells the server (best effort, a few seconds at
    /// most), stops the link, wipes the sealed identity and every web device
    /// row. A re-pair resets the grants on the server to pending, so a stale
    /// confirmed row would otherwise skip the re-confirmation. Blocking.
    /// Safe to repeat.
    pub fn web_unpair(&self) -> Result<(), String> {
        if let Ok(Some(identity)) = self.load_web_identity() {
            if let Ok(base) = desktop_api::base_url(&identity.relay_url, cfg!(debug_assertions)) {
                let _ = desktop_api::delete_me(
                    &base,
                    &identity.desktop_token,
                    desktop_api::UNPAIR_TIMEOUT,
                );
            }
        }
        self.web.stop();
        identity_store::wipe_web(
            &self.web_database()?,
            &identity_store::web_account_for(&self.path),
        )?;
        // Under the same lock the grant sync applies with, so a late sync
        // cannot refill the pending set after the wipe.
        let mut inner = self.inner.lock().map_err(unavailable)?;
        self.web.forget();
        self.remove_web_rows_locked(&mut inner)
    }

    /// Applies the server's grant list (grant sync): pending devices become
    /// the pending set, revoked ones lose their row, a lowered permission is
    /// applied and a raised one becomes a request. Live sessions are told
    /// through the link. Returns what to report to the server.
    pub(super) fn apply_server_grants(
        &self,
        grants: &[desktop_api::ServerGrant],
        generation: u64,
    ) -> Vec<Report> {
        let mut reports: Vec<Report> = Vec::new();
        let mut commands: Vec<LinkCommand> = Vec::new();
        {
            let Ok(mut inner) = self.inner.lock() else {
                return reports;
            };
            // Checked under the lock Unpair also takes: a sync of an unpaired
            // link applies nothing.
            if self.web.generation() != generation {
                return reports;
            }
            let plan = super::web_link::grants::plan(
                grants,
                &inner.config.devices,
                &inner.config.web_revoked,
            );
            let mut config = inner.config.clone();
            config.web_revoked.retain(|id| {
                !plan
                    .settled
                    .iter()
                    .any(|settled| settled.eq_ignore_ascii_case(id))
            });
            let mut closing: Vec<String> = Vec::new();
            for row_id in &plan.revoked_rows {
                if let Some(position) = config.devices.iter().position(|d| &d.id == row_id) {
                    let removed = config.devices.remove(position);
                    closing.push(removed.fingerprint.clone());
                    commands.push(LinkCommand::Revoke {
                        fingerprint: removed.fingerprint,
                    });
                }
            }
            for (row_id, permission) in &plan.lowered {
                if let Some(row) = config.devices.iter_mut().find(|d| &d.id == row_id) {
                    row.permission = *permission;
                    closing.push(row.fingerprint.clone());
                    commands.push(LinkCommand::Resend {
                        fingerprint: row.fingerprint.clone(),
                    });
                    if let Some(server_id) = row.server_device_id.clone() {
                        reports.push((server_id, "confirmed", Some(*permission)));
                    }
                }
            }
            if config.devices != inner.config.devices
                || config.web_revoked != inner.config.web_revoked
            {
                if self.save(&config).is_err() {
                    return Vec::new();
                }
                inner.config = config;
                for fingerprint in &closing {
                    close_device_watches(&mut inner, fingerprint);
                }
            }
            let requests = plan
                .raised
                .iter()
                .filter_map(|(row_id, server_id, requested)| {
                    let row = inner.config.devices.iter().find(|d| &d.id == row_id)?;
                    Some(PermissionRequest {
                        device_id: row.id.clone(),
                        server_device_id: server_id.clone(),
                        name: row.name.clone(),
                        current: row.permission,
                        requested: *requested,
                    })
                })
                .collect();
            self.web.set_requests(requests);
            // The pending set is replaced under the same lock that `web_confirm`
            // takes, so a device confirmed meanwhile is not listed again.
            self.web.pending().replace(plan.pending);
            for server_id in plan.rerevoke {
                reports.push((server_id, "revoked", None));
            }
            for server_id in plan.orphaned {
                reports.push((server_id, "pending", None));
            }
            for (server_id, permission) in plan.reconfirm {
                if !reports.iter().any(|r| r.0 == server_id) {
                    reports.push((server_id, "confirmed", Some(permission)));
                }
            }
        }
        for command in commands {
            self.web.command(command);
        }
        reports
    }
}

impl MobileHost {
    /// Whether the web link is switched on (independent of QUIC access).
    pub fn web_enabled(&self) -> bool {
        self.inner
            .lock()
            .is_ok_and(|inner| inner.config.web_enabled)
    }

    /// Switches the web link on or off, saves the choice and starts or stops
    /// the relay client. It never binds a socket and never touches the router.
    pub fn set_web_enabled(&self, enabled: bool) -> Result<(), String> {
        self.set_web_enabled_with(enabled, super::web_link::relay::Timing::default())
    }

    pub(super) fn set_web_enabled_with(
        &self,
        enabled: bool,
        timing: super::web_link::relay::Timing,
    ) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().map_err(unavailable)?;
            let mut config = inner.config.clone();
            config.web_enabled = enabled;
            self.save(&config)?;
            inner.config = config;
            if !enabled {
                // Only the web watches end: phones are served over QUIC.
                inner
                    .watches
                    .retain(|_, watch| watch.via != DeviceKind::Web);
            }
        }
        super::web_link::sync_with(self, timing);
        Ok(())
    }

    /// The project id of a session in the last published catalog, for the
    /// web push notice. One lock, no I/O.
    pub(crate) fn catalog_session_project(&self, session_id: &str) -> Option<String> {
        let inner = self.inner.lock().ok()?;
        inner.catalog["sessions"]
            .as_array()?
            .iter()
            .find(|entry| entry["id"].as_str() == Some(session_id))
            .and_then(|entry| entry["projectId"].as_str())
            .map(str::to_owned)
    }

    /// The projects and Monos of the last published catalog as `(id, name)`
    /// pairs in catalog order (the `id` and `name` fields the app writes in
    /// `shell/mobile_host.rs`; the ids are what `Device.projects` and
    /// `Device.monos` hold). Empty before anything was published. One lock.
    #[allow(clippy::type_complexity)] // the pair of (id, name) lists is the API
    pub fn catalog_choices(&self) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let Ok(inner) = self.inner.lock() else {
            return Default::default();
        };
        let pairs = |key: &str| -> Vec<(String, String)> {
            inner.catalog[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    let id = entry["id"].as_str()?;
                    let name = entry["name"].as_str().unwrap_or(id);
                    Some((id.to_owned(), name.to_owned()))
                })
                .collect()
        };
        (pairs("projects"), pairs("monos"))
    }

    /// What Settings shows about the web link.
    pub fn web_status(&self) -> super::web_link::WebStatus {
        let (enabled, devices) = self
            .inner
            .lock()
            .map(|inner| {
                let web = inner
                    .config
                    .devices
                    .iter()
                    .filter(|device| device.kind == DeviceKind::Web)
                    .cloned()
                    .collect();
                (inner.config.web_enabled, web)
            })
            .unwrap_or_default();
        self.web.status(enabled, devices)
    }

    pub(super) fn web_link(&self) -> &super::web_link::WebLink {
        &self.web
    }

    /// What the web link knows of a device fingerprint, for a handshake.
    /// Never blocks: one short lock, no I/O.
    pub(super) fn web_lookup(&self, fingerprint: &str) -> super::web_link::session::Lookup {
        self.lookup(fingerprint, None)
    }

    /// `web_lookup` for a handshake: a pending device must also come in under
    /// the relay device id its grant named.
    pub(super) fn web_lookup_for(
        &self,
        fingerprint: &str,
        relay_id: &str,
    ) -> super::web_link::session::Lookup {
        self.lookup(fingerprint, Some(relay_id))
    }

    fn lookup(
        &self,
        fingerprint: &str,
        relay_id: Option<&str>,
    ) -> super::web_link::session::Lookup {
        use super::web_link::session::Lookup;
        if let Ok(inner) = self.inner.lock() {
            if let Some(device) = inner
                .config
                .devices
                .iter()
                .find(|device| device.fingerprint == fingerprint)
            {
                return if device.kind == DeviceKind::Web {
                    Lookup::Confirmed {
                        device: device.clone(),
                    }
                } else {
                    Lookup::Unknown
                };
            }
        }
        let pending = self.web.pending().get(fingerprint).is_some_and(|grant| {
            relay_id.is_none_or(|id| grant.server_device_id.eq_ignore_ascii_case(id))
        });
        if pending {
            Lookup::Pending
        } else {
            Lookup::Unknown
        }
    }

    /// Whether `peer` is still a paired web device. The shared serve asks
    /// before every request and watch frame; the flag `web_enabled` is checked
    /// by `handle_web` and `watch_web` themselves (and turning it off stops the
    /// whole link), so a refused request keeps its "Web access disabled" text.
    pub(super) fn web_admits(&self, peer: &str) -> bool {
        self.inner
            .lock()
            .is_ok_and(|inner| find_device(&inner, DeviceKind::Web, peer).is_ok())
    }

    #[cfg(test)]
    pub(super) fn router_started(&self) -> bool {
        self.inner.lock().is_ok_and(|inner| inner.router_started)
    }

    /// Sets the saved flags in memory only, so a running client keeps running.
    #[cfg(test)]
    pub(super) fn set_flags_for_test(&self, enabled: bool, web_enabled: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.config.enabled = enabled;
        inner.config.web_enabled = web_enabled;
    }

    #[cfg(test)]
    pub(super) fn add_device_for_test(&self, device: Device) {
        let mut inner = self.inner.lock().unwrap();
        inner.config.devices.push(device);
    }
}

/// The outcome to record once a call ended: a call that was admitted and then
/// failed is an error; a denial stays the denial.
fn final_outcome(recorded: Outcome, failed: bool) -> Outcome {
    match recorded {
        Outcome::Ok if failed => Outcome::Error,
        other => other,
    }
}

/// Marks the audited call of the device as denied with `denial`'s code.
fn mark_denied(audited: &mut Option<(DeviceContext, AuditEvent)>, denial: &policy::Denial) {
    if let Some((_, event)) = audited {
        event.outcome = Outcome::Denied(denial.code);
    }
}

/// Whether a device of `kind` may enrol through `handle`. Only QUIC phones do.
fn enroll_allowed(kind: DeviceKind) -> bool {
    kind == DeviceKind::Phone
}

fn addresses(inner: &Inner) -> Vec<Address> {
    let mut result: Vec<Address> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| {
            let ip = interface.ip();
            if ip.is_loopback()
                || ip.is_unspecified()
                || matches!(ip, std::net::IpAddr::V6(v) if v.is_unicast_link_local())
            {
                None
            } else {
                Some(Address {
                    host: ip.to_string(),
                    port: PORT,
                })
            }
        })
        .collect();
    if let Some(address) = &inner.router_address {
        result.insert(0, address.clone());
    }
    if let Ok(address) = Address::parse(&inner.config.remote_address) {
        result.insert(0, address);
    }
    result.dedup();
    result.truncate(16);
    result
}

pub(super) fn name() -> String {
    hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone_row() -> Device {
        Device {
            id: "d".into(),
            name: "Phone".into(),
            fingerprint: "abcdef0123456789".into(),
            push: None,
            kind: DeviceKind::Phone,
            permission: Permission::Control,
            projects: None,
            monos: None,
            server_device_id: None,
            created_at: None,
            last_seen: None,
        }
    }

    /// `mobile-access.json` exactly as the code before the web link wrote it.
    const OLD_CONFIG: &str = r#"{"enabled":true,"openRouter":false,"remoteAddress":"203.0.113.5:4433","devices":[{"id":"d1","name":"iPhone","fingerprint":"aaaa1111bbbb2222","push":{"token":"ExponentPushToken[abc]","disabled":["agentFinished"]}},{"id":"d2","name":"Pixel","fingerprint":"cccc3333dddd4444"}]}"#;

    fn host_in(dir: &std::path::Path) -> MobileHost {
        MobileHost::open(dir.join("mobile-access.json")).unwrap().0
    }

    #[test]
    fn an_old_config_loads_every_device_as_a_controlling_phone_and_saves_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mobile-access.json"), OLD_CONFIG).unwrap();
        let host = host_in(dir.path());
        let config = host.inner.lock().unwrap().config.clone();
        assert_eq!(config.devices.len(), 2);
        for device in &config.devices {
            assert_eq!(device.kind, DeviceKind::Phone);
            assert_eq!(device.permission, Permission::Control);
            assert_eq!((&device.projects, &device.monos), (&None, &None));
            assert!(device.server_device_id.is_none());
            assert!(device.created_at.is_none() && device.last_seen.is_none());
            assert_eq!(device_context(device), DeviceContext::phone(device));
        }
        assert_eq!(
            config.devices[0].push.as_ref().unwrap().disabled,
            ["agentFinished"]
        );
        host.save(&config).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("mobile-access.json")).unwrap(),
            OLD_CONFIG
        );
    }

    #[test]
    fn a_web_device_row_round_trips_with_every_new_field() {
        let dir = tempfile::tempdir().unwrap();
        let host = host_in(dir.path());
        let web = Device {
            id: "w1".into(),
            name: "Browser".into(),
            fingerprint: "eeee5555ffff6666".into(),
            push: None,
            kind: DeviceKind::Web,
            permission: Permission::Read,
            projects: Some(vec!["p1".into()]),
            monos: Some(vec![]),
            server_device_id: Some("srv-1".into()),
            created_at: Some(1_700_000_000),
            last_seen: Some(1_700_000_500),
        };
        let mut config = host.inner.lock().unwrap().config.clone();
        config.devices.push(web.clone());
        host.save(&config).unwrap();
        let text = std::fs::read_to_string(dir.path().join("mobile-access.json")).unwrap();
        assert!(text.contains("\"serverDeviceId\":\"srv-1\""), "{text}");
        let reopened = host_in(dir.path());
        let loaded = reopened.inner.lock().unwrap().config.devices[0].clone();
        assert_eq!(loaded, web);
        let context = device_context(&loaded);
        assert_eq!(context.kind, DeviceKind::Web);
        assert_eq!(context.permission, Permission::Read);
        assert_eq!(context.projects, Some(vec!["p1".to_owned()]));
        assert_eq!(context.monos, Some(vec![]));
        assert_eq!(
            audit_event(&loaded, "workspace.list", &json!({})).transport,
            Transport::Web
        );
        assert_eq!(
            audit_event(&phone_row(), "workspace.list", &json!({})).transport,
            Transport::Quic
        );
    }

    fn host_with_store() -> (tempfile::TempDir, MobileHost) {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("store.db");
        rusqlite::Connection::open(&database).unwrap();
        crate::accounts::register_database(dir.path(), &database);
        let host = host_in(dir.path());
        (dir, host)
    }

    fn web_identity() -> identity_store::WebIdentity {
        identity_store::WebIdentity {
            static_private: [3; 32],
            static_public: [4; 32],
            desktop_id: "desktop-9".into(),
            desktop_token: "test-token-not-real".into(),
            relay_url: "wss://relay.example.test/ws".into(),
            account_handle: None,
        }
    }

    #[test]
    fn the_web_token_never_reaches_the_settings_file_and_unpair_wipes_it() {
        let (dir, host) = host_with_store();
        assert_eq!(host.load_web_identity(), Ok(None));
        host.save_web_identity(&web_identity()).unwrap();
        assert_eq!(host.load_web_identity(), Ok(Some(web_identity())));
        let mut config = host.inner.lock().unwrap().config.clone();
        config.enabled = true;
        host.save(&config).unwrap();
        let bytes = std::fs::read(dir.path().join("mobile-access.json")).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("test-token-not-real"));
        assert!(!text.contains("relay.example.test"));
        host.web_unpair().unwrap();
        assert_eq!(host.load_web_identity(), Ok(None));
        host.web_unpair().unwrap();
    }

    #[test]
    fn the_web_identity_needs_a_registered_store() {
        let dir = tempfile::tempdir().unwrap();
        let host = host_in(dir.path());
        assert!(host.load_web_identity().is_err());
        assert!(host.save_web_identity(&web_identity()).is_err());
        assert!(host.web_unpair().is_err());
    }

    fn fixture() -> (
        MobileHost,
        async_channel::Receiver<Event>,
        Identity,
        Identity,
    ) {
        let join = Identity::generate().unwrap();
        let phone = Identity::generate().unwrap();
        let key = join.fingerprint().unwrap();
        let dir =
            std::env::temp_dir().join(format!("monocode-mobile-test-{}", uuid::Uuid::new_v4()));
        let (host, events) = MobileHost::open(dir.join("mobile-access.json")).unwrap();
        host.gate.allow(key.clone(), Some(now() + 900));
        {
            let mut inner = host.inner.lock().unwrap();
            inner.config.enabled = true;
            inner.identity = Some(Identity::generate().unwrap());
            inner.invitations.insert(
                key,
                Invitation {
                    expires: now() + 900,
                },
            );
        }
        (host, events, join, phone)
    }

    fn check_pending_start_stays_stopped(shutdown: bool) {
        let (host, _events, _join, _phone) = fixture();
        host.inner.lock().unwrap().config.open_router = false;
        let loading = host.identity_load.lock().unwrap();
        let starting = host.clone();
        let (entered, waiting) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            entered.send(()).unwrap();
            starting.start()
        });
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        if shutdown {
            host.shutdown();
        } else {
            host.configure(false, false, "").unwrap();
        }
        drop(loading);
        let result = worker.join().unwrap();
        let listening = host.status().listening;
        host.shutdown();
        if host.path.exists() {
            std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
        }
        result.unwrap();
        assert!(!listening, "listener started after shutdown={shutdown}");
    }

    #[test]
    fn pending_start_does_not_listen_after_disable() {
        check_pending_start_stays_stopped(false);
    }

    #[test]
    fn pending_start_does_not_listen_after_shutdown() {
        check_pending_start_stays_stopped(true);
    }

    #[test]
    fn identity_persists_in_the_store_across_hosts_without_the_keychain() {
        use super::super::identity_store::tests::FakeKeychain;
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("store.db");
        rusqlite::Connection::open(&database).unwrap();
        crate::accounts::register_database(dir.path(), &database);
        let config = dir.path().join("mobile-access.json");
        let first_keychain = Arc::new(FakeKeychain::default());
        let (first, _events) =
            MobileHost::open_with(config.clone(), first_keychain.clone()).unwrap();
        let created = first.identity().unwrap();
        let second_keychain = Arc::new(FakeKeychain::default());
        let (second, _events) = MobileHost::open_with(config, second_keychain.clone()).unwrap();
        let loaded = second.identity().unwrap();
        assert_eq!(
            created.fingerprint().unwrap(),
            loaded.fingerprint().unwrap()
        );
        assert_eq!(second_keychain.gets.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn invitation_is_consumed_once_and_only_public_device_metadata_is_persisted() {
        let (host, _events, join, phone) = fixture();
        let key = join.fingerprint().unwrap();
        let params = json!({"certificate":phone.certificate,"proof":phone.proof(&key).unwrap(),"name":"iPhone"});
        let mut inner = host.inner.lock().unwrap();
        let result = host.enroll(&mut inner, &key, &params).unwrap();
        assert!(result["deviceId"].is_string());
        assert!(!host.gate.admits(&key));
        assert!(host.gate.admits(&phone.fingerprint().unwrap()));
        assert!(host.enroll(&mut inner, &key, &params).is_err());
        let saved = std::fs::read_to_string(&host.path).unwrap();
        assert!(!saved.contains("privateKey") && !saved.contains(&phone.private_key));
        assert_eq!(inner.config.devices.len(), 1);
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_proof_or_expiration_does_not_consume_an_invitation_or_enroll_a_device() {
        let (host, _events, join, phone) = fixture();
        let key = join.fingerprint().unwrap();
        let mut inner = host.inner.lock().unwrap();
        let params = json!({"certificate":phone.certificate,"proof":phone.proof("different-invitation").unwrap()});
        assert!(host.enroll(&mut inner, &key, &params).is_err());
        assert!(inner.config.devices.is_empty() && inner.invitations.contains_key(&key));
        inner.invitations.get_mut(&key).unwrap().expires = now() - 1;
        let params = json!({"certificate":phone.certificate,"proof":phone.proof(&key).unwrap()});
        assert!(host.enroll(&mut inner, &key, &params).is_err());
        assert!(inner.config.devices.is_empty() && !host.path.exists());
    }

    fn paired() -> (MobileHost, async_channel::Receiver<Event>, String) {
        let (host, events, join, phone) = fixture();
        let key = join.fingerprint().unwrap();
        let params = json!({"certificate":phone.certificate,"proof":phone.proof(&key).unwrap()});
        host.enroll(&mut host.inner.lock().unwrap(), &key, &params)
            .unwrap();
        (host, events, phone.fingerprint().unwrap())
    }

    #[tokio::test]
    async fn usage_continuation_commands_reach_the_desktop_with_their_state_tokens() {
        let (host, events, peer) = paired();
        let answering = tokio::spawn(async move {
            for method in ["session.resumeUsage", "session.resumeUsageAtReset"] {
                let Ok(Event::Request(request)) = events.recv().await else {
                    panic!("expected a request");
                };
                assert_eq!(request.method, method);
                assert_eq!(request.params["token"], "paused");
                request.reply(Ok(json!({"sessionId":"s"})));
            }
        });
        for method in ["session.resumeUsage", "session.resumeUsageAtReset"] {
            let result = host.handle(&peer, json!({"version":VERSION,"id":method,"method":method,"params":{"conversationId":"s", "token":"paused", "enabled":true}})).await.unwrap();
            assert_eq!(result["sessionId"], "s");
        }
        answering.await.unwrap();
    }

    #[tokio::test]
    async fn requests_reach_the_app_scoped_to_the_device_and_return_its_answer() {
        let (host, events, peer) = paired();
        let answering = tokio::spawn(async move {
            let Ok(Event::Request(request)) = events.recv().await else {
                panic!("expected a request");
            };
            assert!(request.command_id.ends_with(":c1"));
            assert_eq!(request.method, "session.cancel");
            request.reply(Ok(json!({"sessionId":"s"})));
        });
        let result = host
            .handle(&peer, json!({"version":VERSION,"id":"c1","method":"session.cancel","params":{"conversationId":"s"}}))
            .await
            .unwrap();
        assert_eq!(result["sessionId"], "s");
        answering.await.unwrap();
        assert!(host
            .handle(
                &peer,
                json!({"version":VERSION,"id":"c2","method":"shell.exec","params":{}})
            )
            .await
            .is_err());
        assert!(host
            .handle(
                "stranger",
                json!({"version":VERSION,"id":"c3","method":"workspace.list","params":{}})
            )
            .await
            .is_err());
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn requests_carry_the_phone_context_and_new_methods_reach_the_app() {
        let (host, events, peer) = paired();
        let device = host.inner.lock().unwrap().config.devices[0].clone();
        let answering = tokio::spawn(async move {
            for method in ["session.answerApproval", "cloud.stop"] {
                let Ok(Event::Request(request)) = events.recv().await else {
                    panic!("expected a request");
                };
                assert_eq!(request.method, method);
                assert_eq!(request.device, DeviceContext::phone(&device));
                request.reply(Err("Unsupported mobile operation".into()));
            }
        });
        for method in ["session.answerApproval", "cloud.stop"] {
            let error = host
                .handle(
                    &peer,
                    json!({"version":VERSION,"id":method,"method":method,"params":{}}),
                )
                .await
                .unwrap_err();
            assert_eq!(error, "Unsupported mobile operation");
        }
        answering.await.unwrap();
        // device.selection is the host's and answers as an unknown method did.
        let error = host
            .handle(
                &peer,
                json!({"version":VERSION,"id":"s","method":"device.selection","params":{}}),
            )
            .await
            .unwrap_err();
        assert_eq!(error, "Unsupported mobile operation");
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_recorded_outcome_follows_the_call() {
        assert_eq!(final_outcome(Outcome::Ok, false), Outcome::Ok);
        assert_eq!(final_outcome(Outcome::Ok, true), Outcome::Error);
        let denied = Outcome::Denied(policy::Code::ReadOnly);
        assert_eq!(final_outcome(denied, true), denied);
        let device = Device {
            id: "d".into(),
            name: "Phone".into(),
            fingerprint: "abcdef0123456789".into(),
            push: None,
            ..phone_row()
        };
        let request = json!({"id": "c", "params": {}});
        let mut audited = Some((
            DeviceContext::phone(&device),
            audit_event(&device, "mono.delete", &request),
        ));
        mark_denied(
            &mut audited,
            &policy::Denial {
                code: policy::Code::Forbidden,
                error: "no".into(),
            },
        );
        assert_eq!(
            audited.unwrap().1.outcome,
            Outcome::Denied(policy::Code::Forbidden)
        );
    }

    fn narrowed(
        projects: Option<Vec<String>>,
        monos: Option<Vec<String>>,
    ) -> policy::SelectionAnswer {
        policy::SelectionAnswer {
            reply: json!({"ok": true}),
            narrowed: Some(policy::Selection { projects, monos }),
        }
    }

    fn watch(inner: &mut Inner, id: &str, peer: &str) {
        let (sink, _receiver) = tokio::sync::mpsc::channel::<Value>(1);
        inner.watches.insert(
            id.into(),
            Watcher {
                peer: peer.into(),
                via: DeviceKind::Phone,
                sink,
            },
        );
    }

    #[test]
    fn a_narrower_selection_is_saved_and_closes_the_devices_watches() {
        let (host, _events, peer) = paired();
        {
            let mut inner = host.inner.lock().unwrap();
            watch(&mut inner, "w", &peer);
            watch(&mut inner, "other", "someone-else");
            let reply = host
                .answer_selection(
                    &mut inner,
                    &peer,
                    narrowed(Some(vec!["p".into()]), Some(vec![])),
                )
                .unwrap();
            assert_eq!(reply, json!({"ok": true}));
            assert_eq!(inner.watches.keys().collect::<Vec<_>>(), ["other"]);
            let device = &inner.config.devices[0];
            assert_eq!(device.projects, Some(vec!["p".to_owned()]));
            assert_eq!(device.monos, Some(vec![]));
        }
        let saved: Config = serde_json::from_slice(&std::fs::read(&host.path).unwrap()).unwrap();
        assert_eq!(saved.devices[0].projects, Some(vec!["p".to_owned()]));
        assert_eq!(saved.devices[0].monos, Some(vec![]));
        // The stored selection is what the next request is scoped to.
        let context = device_context(&saved.devices[0]);
        assert_eq!(context.projects, Some(vec!["p".to_owned()]));
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_answer_without_a_narrower_selection_changes_nothing() {
        let (host, _events, peer) = paired();
        let mut inner = host.inner.lock().unwrap();
        watch(&mut inner, "w", &peer);
        let answer = policy::SelectionAnswer {
            reply: json!({"ok": true}),
            narrowed: None,
        };
        assert_eq!(
            host.answer_selection(&mut inner, &peer, answer),
            Ok(json!({"ok": true}))
        );
        assert!(inner.watches.contains_key("w"));
        assert_eq!(inner.config.devices[0].projects, None);
        drop(inner);
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_selection_that_cannot_be_saved_is_refused_and_keeps_the_watches() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(
            &blocker,
            b"a file, so the settings directory cannot be made",
        )
        .unwrap();
        let (host, _events) = MobileHost::open(blocker.join("mobile-access.json")).unwrap();
        let mut inner = host.inner.lock().unwrap();
        inner.config.devices.push(Device {
            fingerprint: "fp".into(),
            ..phone_row()
        });
        watch(&mut inner, "w", "fp");
        assert!(host
            .answer_selection(&mut inner, "fp", narrowed(Some(vec![]), None))
            .is_err());
        assert!(inner.watches.contains_key("w"));
        assert_eq!(inner.config.devices[0].projects, None);
    }

    #[test]
    fn a_refused_selection_change_leaves_the_watches_open() {
        let (host, _events, peer) = paired();
        {
            let mut inner = host.inner.lock().unwrap();
            watch(&mut inner, "w", &peer);
            // No such device: the change is refused.
            assert!(host
                .answer_selection(&mut inner, "stranger", narrowed(Some(vec![]), None))
                .is_err());
            assert!(inner.watches.contains_key("w"));
            assert_eq!(inner.config.devices[0].projects, None);
        }
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn only_quic_phones_enrol_through_the_host() {
        assert!(enroll_allowed(DeviceKind::Phone));
        assert!(!enroll_allowed(DeviceKind::Web));
    }

    #[tokio::test]
    async fn the_audit_event_is_left_for_after_the_call_with_its_outcome() {
        let (host, events, peer) = paired();
        drop(events);
        let mut audited = None;
        let request = json!({"version":VERSION,"id":"c","method":"session.cancel","params":{"projectId":"p"}});
        let result = host.handle_call(&peer, &request, &mut audited).await;
        assert!(result.is_err());
        let (context, event) = audited.unwrap();
        assert_eq!(context.kind, DeviceKind::Phone);
        assert_eq!(event.method, "session.cancel");
        assert_eq!(event.project_id.as_deref(), Some("p"));
        assert_eq!(event.command_id.as_deref(), Some("c"));
        // Admitted, so Ok until the call has ended.
        assert_eq!(event.outcome, Outcome::Ok);
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn closing_a_devices_watches_leaves_other_devices_alone() {
        let (host, _events, peer) = paired();
        {
            let mut inner = host.inner.lock().unwrap();
            for (id, owner) in [
                ("w1", peer.as_str()),
                ("w2", peer.as_str()),
                ("w3", "other"),
            ] {
                let (sink, _receiver) = tokio::sync::mpsc::channel::<Value>(1);
                inner.watches.insert(
                    id.into(),
                    Watcher {
                        peer: owner.into(),
                        via: DeviceKind::Phone,
                        sink,
                    },
                );
            }
            close_device_watches(&mut inner, &peer);
            assert_eq!(inner.watches.keys().collect::<Vec<_>>(), ["w3"]);
        }
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn catalog_choices_lists_published_projects_and_monos_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let host = host_in(dir.path());
        assert_eq!(host.catalog_choices(), (vec![], vec![]));
        host.publish(json!({
            "projects":[{"id":"p2","name":"Beta"},{"id":"p1","name":"Alpha"},{"name":"no id"}],
            "monos":[{"id":"m1","name":"Fox"}],
            "sessions":[]
        }))
        .unwrap();
        let (projects, monos) = host.catalog_choices();
        assert_eq!(
            projects,
            [
                ("p2".to_string(), "Beta".to_string()),
                ("p1".to_string(), "Alpha".to_string())
            ]
        );
        assert_eq!(monos, [("m1".to_string(), "Fox".to_string())]);
    }

    #[tokio::test]
    async fn the_catalog_answers_workspace_polls_without_the_app() {
        let (host, _events, peer) = paired();
        host.publish(
            json!({"projects":[{"id":"p"}],"monos":[],"sessions":[{"id":"s","projectId":"p"}]}),
        )
        .unwrap();
        let catalog = host
            .handle(
                &peer,
                json!({"version":VERSION,"id":"c","method":"workspace.list","params":{}}),
            )
            .await
            .unwrap();
        assert_eq!(catalog["sessions"][0]["id"], "s");
        assert!(catalog["addresses"].is_array());
        assert!(host.publish(json!({"projects":[]})).is_err());
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn a_phone_registers_its_push_token_and_turns_categories_off() {
        let (host, _events, peer) = paired();
        let register = |params: Value| {
            host.handle(
                &peer,
                json!({"version":VERSION,"id":"p","method":"device.push","params":params}),
            )
        };
        assert!(register(json!({"token":"https://example.com"}))
            .await
            .is_err());
        register(json!({"token":"ExponentPushToken[abc]","disabled":["agentFinished"]}))
            .await
            .unwrap();
        assert_eq!(host.push_tokens("agentInput"), ["ExponentPushToken[abc]"]);
        assert!(host.push_tokens("agentFinished").is_empty());
        let saved = std::fs::read_to_string(&host.path).unwrap();
        assert!(saved.contains("ExponentPushToken[abc]"));
        host.forget_push_tokens(&["ExponentPushToken[abc]".into()])
            .unwrap();
        assert!(host.push_tokens("agentInput").is_empty());
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    fn web_row(id: &str, fingerprint: &str) -> Device {
        Device {
            id: id.into(),
            name: "Laptop".into(),
            fingerprint: fingerprint.into(),
            kind: DeviceKind::Web,
            permission: Permission::Read,
            server_device_id: Some("srv-1".into()),
            ..phone_row()
        }
    }

    fn watch_via(inner: &mut Inner, id: &str, peer: &str, via: DeviceKind) {
        let (sink, _receiver) = tokio::sync::mpsc::channel::<Value>(1);
        inner.watches.insert(
            id.into(),
            Watcher {
                peer: peer.into(),
                via,
                sink,
            },
        );
    }

    #[test]
    fn the_catalog_stays_published_while_either_transport_is_on() {
        let (host, _events, _peer) = paired();
        host.set_flags_for_test(false, false);
        assert!(!host.enabled());
        host.set_flags_for_test(true, false);
        assert!(host.enabled());
        // Web only: the app still publishes, but Settings' QUIC switch is off.
        host.set_flags_for_test(false, true);
        assert!(host.enabled());
        assert!(!host.status().enabled);
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn the_web_transport_gets_no_push_registration_and_no_push_tokens() {
        let (host, _events, phone) = paired();
        host.set_flags_for_test(true, true);
        host.add_device_for_test(web_row("w", "web-fingerprint"));
        let error = host
            .handle_web(
                "web-fingerprint",
                json!({"version":VERSION,"id":"p","method":"device.push","params":{"token":"ExponentPushToken[web]"}}),
            )
            .await
            .unwrap_err();
        assert_eq!(error, "Unsupported mobile operation");
        assert!(host
            .inner
            .lock()
            .unwrap()
            .config
            .devices
            .iter()
            .all(|device| device.kind != DeviceKind::Web || device.push.is_none()));
        // A web row that somehow carries a token is still never notified.
        {
            let mut inner = host.inner.lock().unwrap();
            let web = inner
                .config
                .devices
                .iter_mut()
                .find(|device| device.kind == DeviceKind::Web)
                .unwrap();
            web.push = Some(PushRegistration {
                token: "ExponentPushToken[web]".into(),
                disabled: Vec::new(),
            });
        }
        assert!(host.push_tokens("agentInput").is_empty());
        host.handle(
            &phone,
            json!({"version":VERSION,"id":"p","method":"device.push","params":{"token":"ExponentPushToken[phone]"}}),
        )
        .await
        .unwrap();
        assert_eq!(host.push_tokens("agentInput"), ["ExponentPushToken[phone]"]);
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn workspace_list_gives_a_web_device_no_addresses_and_a_phone_its_addresses() {
        let (host, _events, phone) = paired();
        host.set_flags_for_test(true, true);
        host.add_device_for_test(web_row("w", "web-fingerprint"));
        let list = json!({"version":VERSION,"id":"c","method":"workspace.list","params":{}});
        let web = host
            .handle_web("web-fingerprint", list.clone())
            .await
            .unwrap();
        assert!(web.get("addresses").is_none(), "{web}");
        assert!(web["projects"].is_array());
        let phone = host.handle(&phone, list).await.unwrap();
        assert!(phone["addresses"].is_array());
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn turning_one_transport_off_ends_only_its_own_watches() {
        let (host, _events, phone) = paired();
        host.set_flags_for_test(true, true);
        host.add_device_for_test(web_row("w", "web-fingerprint"));
        let open = |host: &MobileHost| {
            let mut inner = host.inner.lock().unwrap();
            inner.watches.clear();
            watch_via(&mut inner, "phone", &phone, DeviceKind::Phone);
            watch_via(&mut inner, "web", "web-fingerprint", DeviceKind::Web);
        };
        let keys = |host: &MobileHost| {
            let mut keys: Vec<String> =
                host.inner.lock().unwrap().watches.keys().cloned().collect();
            keys.sort();
            keys
        };
        open(&host);
        host.configure(false, false, "").unwrap();
        assert_eq!(keys(&host), ["web"]);
        open(&host);
        host.set_web_enabled(false).unwrap();
        assert_eq!(keys(&host), ["phone"]);
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }

    #[test]
    fn only_phone_rows_reach_the_quic_gate_when_the_host_opens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mobile-access.json"),
            r#"{"enabled":false,"devices":[{"id":"p","name":"Phone","fingerprint":"phone-fingerprint"},{"id":"w","name":"Laptop","fingerprint":"web-fingerprint","kind":"web","serverDeviceId":"srv-1"}]}"#,
        )
        .unwrap();
        let host = host_in(dir.path());
        assert!(host.gate.admits("phone-fingerprint"));
        assert!(!host.gate.admits("web-fingerprint"));
    }

    #[tokio::test]
    async fn revoking_and_leaving_remove_only_the_row_of_their_own_kind() {
        let (host, _events, phone) = paired();
        host.set_flags_for_test(true, true);
        let phone_id = host.inner.lock().unwrap().config.devices[0].id.clone();
        // A web row with the same id as the phone's.
        host.add_device_for_test(web_row(&phone_id, "web-fingerprint"));
        let leave = json!({"version":VERSION,"id":"l","method":"device.leave","params":{}});
        host.handle_web("web-fingerprint", leave).await.unwrap();
        {
            let inner = host.inner.lock().unwrap();
            let kinds: Vec<_> = inner.config.devices.iter().map(|d| d.kind).collect();
            assert_eq!(kinds, [DeviceKind::Phone]);
        }
        assert!(host.gate.admits(&phone), "the phone stays admitted");
        // And the other way: revoking by the shared id removes one row, and
        // never touches the QUIC gate for a web row.
        host.add_device_for_test(web_row(&phone_id, "web-fingerprint"));
        host.revoke(&phone_id).unwrap();
        {
            let inner = host.inner.lock().unwrap();
            let kinds: Vec<_> = inner.config.devices.iter().map(|d| d.kind).collect();
            assert_eq!(kinds, [DeviceKind::Web]);
        }
        assert!(!host.gate.admits(&phone));
        std::fs::remove_dir_all(host.path.parent().unwrap()).unwrap();
    }
}
