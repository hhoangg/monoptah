//! The per-`(deviceId, channel)` Noise sessions of the web link.
//!
//! [`SessionTable`] is a pure, synchronous, clock-injected state machine: it
//! takes relay frames and the current time in milliseconds and returns
//! [`Action`]s for the caller to carry out (send a frame, hand a decrypted
//! request to the host, tell the UI a device is waiting). It owns no socket,
//! no task and no timer, so every rule is unit-testable. Architecture 5.1 to
//! 5.5 describe the protocol it implements.
//!
//! Nothing here logs frame bytes, plaintext or key material.

use std::collections::HashMap;

use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::frame::{
    split_message, Frame, FrameType, Plaintext, Pushed, Reassembler, MAX_REQUEST_BYTES,
};
use super::noise::{fingerprint, is_low_order, Responder, Transport};
use crate::mobile::host::Device;
use crate::mobile::policy::{self, DeviceKind, PolicyFlags};

/// Channels one device may hold at once (architecture 4.4).
pub const MAX_CHANNELS_PER_DEVICE: usize = 4;
/// Sessions held at once across all devices.
pub const MAX_SESSIONS: usize = 64;
/// Pending (unconfirmed) sessions one device key may hold at once.
pub const MAX_PENDING_PER_FINGERPRINT: usize = 4;
/// Pending sessions held at once across all device keys.
pub const MAX_PENDING_TOTAL: usize = 16;
/// The name shown for a device whose name is empty once cleaned.
pub const UNNAMED_DEVICE: &str = "Unnamed device";
/// A pending session is closed after this long without confirmation.
pub const PENDING_TIMEOUT_MS: u64 = 5 * 60 * 1000;
/// A session with no frame in either direction for this long is closed.
pub const IDLE_LIMIT_MS: u64 = 12 * 60 * 60 * 1000;
/// Rekey after this many messages sent since the last rekey.
pub const REKEY_AFTER_MESSAGES: u64 = 65_536;
/// Rekey after this long since the last rekey (or the session start).
pub const REKEY_AFTER_MS: u64 = 60 * 60 * 1000;
/// Longest device name taken from the untrusted HS1 payload, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// The protocol version advertised in HS2 and the state message.
pub const PROTO: u32 = 1;

/// Identifies one session: the relay device id and the device-chosen channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    pub device_id: [u8; 16],
    pub channel: u16,
}

/// What the desktop knows about a device fingerprint. Unknown and revoked
/// devices look the same: there is no tombstone (architecture 6.3).
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // short-lived value, boxing only hurts callers
pub enum Lookup {
    Confirmed { device: Device },
    Pending,
    Unknown,
}

/// Answers a lookup for a device fingerprint and the relay deviceId (lowercase
/// hyphenated UUID, the form `Device::server_device_id` is stored in).
pub type Directory = Box<dyn Fn(&str, &str) -> Lookup + Send + Sync>;

/// Something the caller must do after a frame or a tick.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // short-lived value, boxing only hurts callers
pub enum Action {
    /// Send this frame to the relay.
    Send(Frame),
    /// A complete decrypted JSON envelope from a ready session.
    Deliver {
        key: SessionKey,
        /// The device fingerprint.
        peer: String,
        device: Device,
        envelope: Value,
    },
    /// A pending session finished its handshake. `name` is untrusted text.
    Pending {
        key: SessionKey,
        peer: String,
        name: String,
    },
    /// The session is gone (a BYE was sent unless the device sent it).
    Closed { key: SessionKey },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// No session for the key.
    UnknownSession,
    /// The session is still pending, so nothing may be sent to it.
    NotReady,
    /// The envelope is above the 8 MiB limit.
    TooLarge,
    /// Encryption failed; the session was dropped, so send a BYE
    /// (see [`SessionTable::bye_frame`]).
    Encrypt,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownSession => "no such session",
            Self::NotReady => "session is not confirmed",
            Self::TooLarge => "message too large",
            Self::Encrypt => "encryption failed",
        })
    }
}

impl std::error::Error for SendError {}

enum Mode {
    Ready(Box<Device>),
    Pending { since: u64 },
}

struct Session {
    transport: Transport,
    mode: Mode,
    fingerprint: String,
    reassembler: Reassembler,
    last_activity: u64,
    last_rekey: u64,
    next_msg_id: u32,
}

/// Writes the audit line of a confirmed device's session opening or ending. A
/// pending session is not a connected device and writes nothing.
fn audit_link(session: &Session, connected: bool) {
    let Mode::Ready(device) = &session.mode else {
        return;
    };
    crate::mobile::audit::record_lifecycle(
        if connected {
            crate::mobile::audit::Lifecycle::Connect
        } else {
            crate::mobile::audit::Lifecycle::Disconnect
        },
        &device.id,
        &device.fingerprint,
        crate::mobile::audit::Transport::Web,
        None,
    );
}

/// The sessions of one relay connection.
///
/// The desktop static private key is held in a [`Zeroizing`] wrapper and wiped
/// on drop. Residual: snow 0.10 copies the key into its own state for every
/// handshake and does not zeroize its internal key copies (nor the transport
/// cipher keys) when they are freed, so those copies outlive our wrapper until
/// the memory is reused.
pub struct SessionTable {
    static_private: Zeroizing<[u8; 32]>,
    desktop_id: [u8; 16],
    desktop_name: String,
    desktop_version: String,
    directory: Directory,
    sessions: HashMap<SessionKey, Session>,
    rekey_after_messages: u64,
}

/// Cleans the untrusted name of a device for display: control characters, bidi
/// controls and zero-width characters go, whitespace collapses, and at most
/// [`MAX_NAME_CHARS`] characters stay. An empty result is [`UNNAMED_DEVICE`].
pub fn clean_name(raw: &str) -> String {
    let visible = |c: &char| {
        !(c.is_control()
            || matches!(*c,
                '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'))
    };
    let kept: String = raw.chars().filter(visible).collect();
    let collapsed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped: String = collapsed.chars().take(MAX_NAME_CHARS).collect();
    let name = capped.trim();
    if name.is_empty() {
        UNNAMED_DEVICE.to_owned()
    } else {
        name.to_owned()
    }
}

fn relay_device_id(id: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*id).hyphenated().to_string()
}

fn bye(key: SessionKey) -> Frame {
    Frame {
        kind: FrameType::Bye,
        channel: key.channel,
        device_id: key.device_id,
        body: Vec::new(),
    }
}

/// Encrypts one envelope into TRANSPORT frames (a MSG or CHUNKs), rekeying
/// first when a limit is reached. Every body is at most 65,535 bytes because
/// each plaintext is at most 65,519 bytes and the tag adds 16.
fn encrypt_envelope(
    session: &mut Session,
    key: SessionKey,
    envelope: &Value,
    now: u64,
    rekey_after_messages: u64,
) -> Result<Vec<Frame>, SendError> {
    let bytes = serde_json::to_vec(envelope).map_err(|_| SendError::Encrypt)?;
    let msg_id = session.next_msg_id;
    session.next_msg_id = session.next_msg_id.wrapping_add(1);
    let parts =
        split_message(&bytes, msg_id, MAX_REQUEST_BYTES).map_err(|_| SendError::TooLarge)?;
    let mut frames = Vec::with_capacity(parts.len());
    let transport_frame = |body: Vec<u8>| Frame {
        kind: FrameType::Transport,
        channel: key.channel,
        device_id: key.device_id,
        body,
    };
    for part in parts {
        let due = session.transport.sent_count() >= rekey_after_messages
            || now.saturating_sub(session.last_rekey) >= REKEY_AFTER_MS;
        if due {
            let rekey = Plaintext::Rekey.encode().map_err(|_| SendError::Encrypt)?;
            let body = session
                .transport
                .encrypt(&rekey)
                .map_err(|_| SendError::Encrypt)?;
            frames.push(transport_frame(body));
            session.transport.rekey_outgoing();
            session.last_rekey = now;
        }
        let body = session
            .transport
            .encrypt(&part)
            .map_err(|_| SendError::Encrypt)?;
        frames.push(transport_frame(body));
    }
    session.last_activity = now;
    Ok(frames)
}

impl SessionTable {
    pub fn new(
        static_private: impl Into<Zeroizing<[u8; 32]>>,
        desktop_id: [u8; 16],
        desktop_name: impl Into<String>,
        desktop_version: impl Into<String>,
        directory: Directory,
    ) -> Self {
        Self {
            static_private: static_private.into(),
            desktop_id,
            desktop_name: desktop_name.into(),
            desktop_version: desktop_version.into(),
            directory,
            sessions: HashMap::new(),
            rekey_after_messages: REKEY_AFTER_MESSAGES,
        }
    }

    /// Lowers the message-count rekey threshold so tests need not send 65,536
    /// messages.
    #[cfg(test)]
    fn set_rekey_after_messages(&mut self, n: u64) {
        self.rekey_after_messages = n;
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Removes a session and writes its disconnect line when it was ready.
    /// False when there was none.
    fn remove_session(&mut self, key: &SessionKey) -> bool {
        match self.sessions.remove(key) {
            Some(session) => {
                audit_link(&session, false);
                true
            }
            None => false,
        }
    }

    /// A BYE frame for `key`.
    pub fn bye_frame(key: SessionKey) -> Frame {
        bye(key)
    }

    /// Handles one frame from the relay at time `now_ms`.
    pub fn on_frame(&mut self, frame: Frame, now_ms: u64) -> Vec<Action> {
        let key = SessionKey {
            device_id: frame.device_id,
            channel: frame.channel,
        };
        match frame.kind {
            FrameType::Hs1 => self.on_hs1(key, &frame.body, now_ms),
            FrameType::Transport => self.on_transport(key, &frame.body, now_ms),
            FrameType::Bye => {
                if self.remove_session(&key) {
                    vec![Action::Closed { key }]
                } else {
                    Vec::new()
                }
            }
            // HS2 only flows desktop to device: from the relay it is a
            // protocol error.
            FrameType::Hs2 => self.drop_with_bye(key),
            // The relay client handles CTRL and ERROR.
            FrameType::Ctrl | FrameType::Error => Vec::new(),
        }
    }

    fn drop_with_bye(&mut self, key: SessionKey) -> Vec<Action> {
        let mut out = vec![Action::Send(bye(key))];
        if self.remove_session(&key) {
            out.push(Action::Closed { key });
        }
        out
    }

    fn on_hs1(&mut self, key: SessionKey, body: &[u8], now: u64) -> Vec<Action> {
        // The session this key already has is not counted: a new handshake
        // replaces it, but only once that handshake succeeded, so an HS1 that
        // fails leaves the live session as it was.
        let on_device = self
            .sessions
            .keys()
            .filter(|k| k.device_id == key.device_id && **k != key)
            .count();
        let total = self.sessions.len() - usize::from(self.sessions.contains_key(&key));
        if on_device >= MAX_CHANNELS_PER_DEVICE || total >= MAX_SESSIONS {
            return vec![Action::Send(bye(key))];
        }
        let live = self.sessions.contains_key(&key);
        self.handshake(key, body, now).unwrap_or_else(|| {
            // BYE answers an unknown pair. A pair with a live session sends
            // nothing on a failed HS1: whoever sent it (the relay, or a thief
            // of the device id) must not be able to end that session, on the
            // desktop or, through the BYE, in the browser.
            if live {
                Vec::new()
            } else {
                vec![Action::Send(bye(key))]
            }
        })
    }

    /// Runs the responder and the directory check. `None` means BYE with no
    /// state.
    fn handshake(&mut self, key: SessionKey, body: &[u8], now: u64) -> Option<Vec<Action>> {
        let mut responder = Responder::new(
            &self.static_private,
            &self.desktop_id,
            &key.device_id,
            key.channel,
        )
        .ok()?;
        let (payload, remote) = responder.read_hs1(body).ok()?;
        // A small-order "key" makes the shared secrets a constant anyone can
        // compute, so no handshake under it may complete.
        if is_low_order(&remote) {
            return None;
        }
        let hello: Value = serde_json::from_slice(&payload).ok()?;
        let name = clean_name(hello.get("name")?.as_str()?);
        let fp = fingerprint(&remote);
        let relay_id = relay_device_id(&key.device_id);
        let mode = match (self.directory)(&fp, &relay_id) {
            Lookup::Confirmed { device } => {
                // A web row must name the relay device id it was confirmed
                // under; a row without one is not reachable at all.
                let id_matches = device.server_device_id.as_deref() == Some(relay_id.as_str());
                if device.kind != DeviceKind::Web || !id_matches {
                    return None;
                }
                Mode::Ready(Box::new(device))
            }
            Lookup::Pending => {
                // Other keys' sessions only: this key's own old session is
                // replaced if the handshake succeeds.
                let pending = |(k, s): &(&SessionKey, &Session)| {
                    **k != key && matches!(s.mode, Mode::Pending { .. })
                };
                let total = self.sessions.iter().filter(pending).count();
                let own = self
                    .sessions
                    .iter()
                    .filter(|entry| pending(entry) && entry.1.fingerprint == fp)
                    .count();
                if total >= MAX_PENDING_TOTAL || own >= MAX_PENDING_PER_FINGERPRINT {
                    return None;
                }
                Mode::Pending { since: now }
            }
            Lookup::Unknown => return None,
        };
        let hs2_payload = serde_json::to_vec(&json!({ "proto": PROTO })).ok()?;
        let (hs2, transport) = responder.write_hs2(&hs2_payload).ok()?;
        let mut session = Session {
            transport,
            mode,
            fingerprint: fp.clone(),
            reassembler: Reassembler::default(),
            last_activity: now,
            last_rekey: now,
            next_msg_id: 0,
        };
        let state = self.state_message(&session.mode);
        let state_frames =
            encrypt_envelope(&mut session, key, &state, now, self.rekey_after_messages).ok()?;
        let pending = matches!(session.mode, Mode::Pending { .. });
        let mut out = vec![Action::Send(Frame {
            kind: FrameType::Hs2,
            channel: key.channel,
            device_id: key.device_id,
            body: hs2,
        })];
        out.extend(state_frames.into_iter().map(Action::Send));
        if pending {
            out.push(Action::Pending {
                key,
                peer: fp,
                name,
            });
        }
        audit_link(&session, true);
        if let Some(old) = self.sessions.insert(key, session) {
            // The old session for the key ends with the new one's start.
            audit_link(&old, false);
            out.insert(0, Action::Closed { key });
        }
        Some(out)
    }

    fn state_message(&self, mode: &Mode) -> Value {
        self.state_message_with(mode, &policy::flags())
    }

    /// `state_message` for the given toggles: the capabilities follow them.
    fn state_message_with(&self, mode: &Mode, flags: &PolicyFlags) -> Value {
        let capabilities = policy::capabilities(flags);
        let desktop = json!({ "name": self.desktop_name, "version": self.desktop_version });
        match mode {
            Mode::Ready(device) => json!({
                "type": "state",
                "state": "confirmed",
                "permission": match device.permission {
                    policy::Permission::Read => "read",
                    policy::Permission::Control => "control",
                },
                "proto": PROTO,
                "capabilities": capabilities,
                "desktop": desktop,
            }),
            Mode::Pending { .. } => json!({
                "type": "state",
                "state": "pending",
                "proto": PROTO,
                "capabilities": capabilities,
                "desktop": desktop,
            }),
        }
    }

    fn on_transport(&mut self, key: SessionKey, body: &[u8], now: u64) -> Vec<Action> {
        let Some(session) = self.sessions.get_mut(&key) else {
            return vec![Action::Send(bye(key))];
        };
        if now.saturating_sub(session.last_activity) >= IDLE_LIMIT_MS {
            return self.drop_with_bye(key);
        }
        let Ok(plain) = session.transport.decrypt(body) else {
            return self.drop_with_bye(key);
        };
        session.last_activity = now;
        if matches!(session.mode, Mode::Pending { .. }) {
            // The plaintext was decrypted to keep the nonces in step, and is
            // dropped: an unconfirmed device buffers nothing and sends nothing
            // the desktop acts on. Only a rekey has an effect.
            return match Plaintext::decode(&plain) {
                Ok(Plaintext::Rekey) => {
                    session.transport.rekey_incoming();
                    Vec::new()
                }
                Ok(_) => Vec::new(),
                Err(_) => self.drop_with_bye(key),
            };
        }
        match session.reassembler.push(&plain, now) {
            Err(_) => self.drop_with_bye(key),
            Ok(Pushed::Rekey) => {
                session.transport.rekey_incoming();
                Vec::new()
            }
            Ok(Pushed::Partial) => Vec::new(),
            Ok(Pushed::Message(bytes)) => {
                let Ok(envelope) = serde_json::from_slice::<Value>(&bytes) else {
                    return self.drop_with_bye(key);
                };
                match &session.mode {
                    Mode::Ready(device) => vec![Action::Deliver {
                        key,
                        peer: session.fingerprint.clone(),
                        device: (**device).clone(),
                        envelope,
                    }],
                    // Requests from a pending session are ignored.
                    Mode::Pending { .. } => Vec::new(),
                }
            }
        }
    }

    /// Encrypts a desktop to device envelope for a ready session: one MSG, or
    /// CHUNKs when it does not fit. Rekeys first when a limit is reached.
    pub fn send(
        &mut self,
        key: SessionKey,
        envelope: &Value,
        now_ms: u64,
    ) -> Result<Vec<Frame>, SendError> {
        let threshold = self.rekey_after_messages;
        let session = self
            .sessions
            .get_mut(&key)
            .ok_or(SendError::UnknownSession)?;
        if !matches!(session.mode, Mode::Ready(_)) {
            return Err(SendError::NotReady);
        }
        let result = encrypt_envelope(session, key, envelope, now_ms, threshold);
        if result == Err(SendError::Encrypt) {
            self.remove_session(&key);
        }
        result
    }

    /// Handles timeouts: idle sessions, unconfirmed pending sessions and
    /// reassemblies older than 60 s are closed with a BYE.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Action> {
        let mut expired: Vec<SessionKey> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                let idle = now_ms.saturating_sub(s.last_activity) >= IDLE_LIMIT_MS;
                let pending = matches!(
                    s.mode,
                    Mode::Pending { since } if now_ms.saturating_sub(since) >= PENDING_TIMEOUT_MS
                );
                idle || pending || s.reassembler.check_timeouts(now_ms).is_err()
            })
            .map(|(k, _)| *k)
            .collect();
        expired.sort();
        expired
            .into_iter()
            .flat_map(|key| self.drop_with_bye(key))
            .collect()
    }

    /// Closes every channel of a device (revoke, or the relay says it left).
    pub fn close_device(&mut self, device_id: &[u8; 16]) -> Vec<Action> {
        let mut keys: Vec<SessionKey> = self
            .sessions
            .keys()
            .filter(|k| &k.device_id == device_id)
            .copied()
            .collect();
        keys.sort();
        keys.into_iter()
            .flat_map(|k| self.drop_with_bye(k))
            .collect()
    }

    /// Closes every session (relay link lost or the desktop stops).
    pub fn close_all(&mut self) -> Vec<Action> {
        let mut keys: Vec<SessionKey> = self.sessions.keys().copied().collect();
        keys.sort();
        keys.into_iter()
            .flat_map(|k| self.drop_with_bye(k))
            .collect()
    }

    /// The sessions of the device with this fingerprint.
    pub fn live_channels(&self, fingerprint: &str) -> Vec<SessionKey> {
        let mut keys: Vec<SessionKey> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.fingerprint == fingerprint)
            .map(|(k, _)| *k)
            .collect();
        keys.sort();
        keys
    }

    /// Every ready session with its device's permission, in key order.
    pub fn ready_sessions(&self) -> Vec<(SessionKey, policy::Permission)> {
        let mut ready: Vec<(SessionKey, policy::Permission)> = self
            .sessions
            .iter()
            .filter_map(|(k, s)| match &s.mode {
                Mode::Ready(device) => Some((*k, device.permission)),
                Mode::Pending { .. } => None,
            })
            .collect();
        ready.sort_by_key(|(key, _)| *key);
        ready
    }

    /// Moves a pending session to ready and sends the confirmed state. Does
    /// nothing for a session that is not pending.
    pub fn confirm(&mut self, key: SessionKey, device: Device, now_ms: u64) -> Vec<Action> {
        let threshold = self.rekey_after_messages;
        let state = self.state_message(&Mode::Ready(Box::new(device.clone())));
        let Some(session) = self.sessions.get_mut(&key) else {
            return Vec::new();
        };
        if !matches!(session.mode, Mode::Pending { .. }) {
            return Vec::new();
        }
        session.mode = Mode::Ready(Box::new(device));
        audit_link(session, true);
        match encrypt_envelope(session, key, &state, now_ms, threshold) {
            Ok(frames) => frames.into_iter().map(Action::Send).collect(),
            Err(_) => self.drop_with_bye(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::frame::{Pushed, MAX_NOISE_PLAINTEXT};
    use super::super::noise::{generate_static, Initiator};
    use super::*;
    use crate::mobile::policy::Permission;
    use std::sync::{Arc, Mutex};

    const DESKTOP: [u8; 16] = [9u8; 16];

    type Dir = Arc<Mutex<HashMap<String, Lookup>>>;

    fn device_row(fp: &str, kind: DeviceKind, server: Option<&str>) -> Device {
        Device {
            id: "dev-1".into(),
            name: "Laptop".into(),
            fingerprint: fp.into(),
            push: None,
            kind,
            permission: Permission::Read,
            projects: None,
            monos: None,
            server_device_id: server.map(str::to_string),
            created_at: None,
            last_seen: None,
        }
    }

    struct Dev {
        private: [u8; 32],
        public: [u8; 32],
        id: [u8; 16],
    }

    impl Dev {
        fn new(id: u8) -> Self {
            let (private, public) = generate_static().unwrap();
            Self {
                private,
                public,
                id: [id; 16],
            }
        }
        fn fp(&self) -> String {
            fingerprint(&self.public)
        }
        fn key(&self, channel: u16) -> SessionKey {
            SessionKey {
                device_id: self.id,
                channel,
            }
        }
    }

    struct H {
        table: SessionTable,
        dir: Dir,
        desktop_pub: [u8; 32],
    }

    fn h() -> H {
        let (private, desktop_pub) = generate_static().unwrap();
        let dir: Dir = Arc::new(Mutex::new(HashMap::new()));
        let d = dir.clone();
        let table = SessionTable::new(
            private,
            DESKTOP,
            "Test Desktop",
            "1.2.3",
            Box::new(move |fp, _relay| {
                d.lock()
                    .unwrap()
                    .get(fp)
                    .cloned()
                    .unwrap_or(Lookup::Unknown)
            }),
        );
        H {
            table,
            dir,
            desktop_pub,
        }
    }

    fn frame(kind: FrameType, key: SessionKey, body: Vec<u8>) -> Frame {
        Frame {
            kind,
            channel: key.channel,
            device_id: key.device_id,
            body,
        }
    }

    fn sent(actions: &[Action]) -> Vec<Frame> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send(f) => Some(f.clone()),
                _ => None,
            })
            .collect()
    }

    fn is_single_bye(actions: &[Action], key: SessionKey) -> bool {
        actions == [Action::Send(bye(key))]
    }

    impl H {
        fn allow(&self, dev: &Dev, lookup: Lookup) {
            self.dir.lock().unwrap().insert(dev.fp(), lookup);
        }

        fn confirmed(&self, dev: &Dev) {
            let server = relay_device_id(&dev.id);
            self.allow(
                dev,
                Lookup::Confirmed {
                    device: device_row(&dev.fp(), DeviceKind::Web, Some(&server)),
                },
            );
        }

        /// Runs the handshake; returns the device transport, the decrypted
        /// messages the desktop sent after HS2, and all actions.
        fn connect(
            &mut self,
            dev: &Dev,
            channel: u16,
            now: u64,
        ) -> (Transport, Vec<Value>, Vec<Action>) {
            let key = dev.key(channel);
            let mut init =
                Initiator::new(&dev.private, &self.desktop_pub, &DESKTOP, &dev.id, channel)
                    .unwrap();
            let hs1 = init.write_hs1(br#"{"name":"Laptop","ua":"test"}"#).unwrap();
            let actions = self.table.on_frame(frame(FrameType::Hs1, key, hs1), now);
            let frames = sent(&actions);
            assert_eq!(frames[0].kind, FrameType::Hs2, "{actions:?}");
            let (payload, mut it) = init.read_hs2(&frames[0].body).unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&payload).unwrap(),
                json!({"proto": 1})
            );
            let msgs = frames[1..]
                .iter()
                .map(|f| {
                    let plain = it.decrypt(&f.body).unwrap();
                    let Plaintext::Msg(m) = Plaintext::decode(&plain).unwrap() else {
                        panic!("expected MSG");
                    };
                    serde_json::from_slice(&m).unwrap()
                })
                .collect();
            (it, msgs, actions)
        }
    }

    fn msg_frame(it: &mut Transport, key: SessionKey, envelope: &Value) -> Frame {
        let plain = Plaintext::Msg(serde_json::to_vec(envelope).unwrap())
            .encode()
            .unwrap();
        frame(FrameType::Transport, key, it.encrypt(&plain).unwrap())
    }

    fn chunk_frame(
        it: &mut Transport,
        key: SessionKey,
        msg_id: u32,
        index: u16,
        last: bool,
        data: &[u8],
    ) -> Frame {
        let plain = Plaintext::Chunk {
            msg_id,
            index,
            last,
            data: data.to_vec(),
        }
        .encode()
        .unwrap();
        frame(FrameType::Transport, key, it.encrypt(&plain).unwrap())
    }

    fn assert_closed_with_bye(actions: &[Action], key: SessionKey) {
        assert_eq!(
            actions,
            [Action::Send(bye(key)), Action::Closed { key }],
            "{actions:?}"
        );
    }

    use crate::mobile::policy::TEST_SETTINGS_LOCK as FLAGS;

    /// S1: the `state` message a web device receives carries what the desktop
    /// supports: `cloud` always, and `cloudStart` while that toggle is on.
    #[test]
    fn the_state_message_carries_cloud_and_follows_the_toggles() {
        let _serial = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        let before = policy::flags();
        let capabilities = |h: &mut H| {
            let dev = Dev::new(2);
            h.confirmed(&dev);
            let (_, msgs, _) = h.connect(&dev, 7, 1_000);
            msgs[0]["capabilities"].clone()
        };
        policy::set_flags(policy::PolicyFlags::default());
        assert_eq!(capabilities(&mut h()), json!(["cloud"]));
        policy::set_flags(policy::PolicyFlags {
            remote_cloud_start: true,
            ..Default::default()
        });
        assert_eq!(capabilities(&mut h()), json!(["cloud", "cloudStart"]));
        policy::set_flags(before);
    }

    /// A toggle change re-sends the state to the ready sessions only, and the
    /// state they get then lists `approvals` while the toggle is on.
    #[test]
    fn ready_sessions_get_a_state_that_follows_the_approvals_toggle() {
        let _serial = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        let before = policy::flags();
        let mut h = h();
        let dev = Dev::new(3);
        h.confirmed(&dev);
        let _ = h.connect(&dev, 7, 1_000);
        assert_eq!(
            h.table.ready_sessions(),
            vec![(dev.key(7), Permission::Read)]
        );
        let mode = Mode::Pending { since: 0 };
        policy::set_flags(PolicyFlags::default());
        assert_eq!(
            h.table.state_message(&mode)["capabilities"],
            json!(["cloud"])
        );
        policy::set_flags(PolicyFlags {
            remote_approvals: true,
            ..Default::default()
        });
        assert_eq!(
            h.table.state_message(&mode)["capabilities"],
            json!(["cloud", "approvals"])
        );
        policy::set_flags(before);
    }

    #[test]
    fn confirmed_handshake_state_and_deliver() {
        let _serial = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        policy::set_flags(policy::PolicyFlags::default());
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(7);
        let (mut it, msgs, _) = h.connect(&dev, 7, 1_000);
        assert_eq!(
            msgs,
            vec![json!({
                "type": "state",
                "state": "confirmed",
                "permission": "read",
                "proto": 1,
                "capabilities": ["cloud"],
                "desktop": {"name": "Test Desktop", "version": "1.2.3"},
            })]
        );
        let req = json!({"version": 1, "rid": 7, "id": "a", "method": "workspace.list"});
        let actions = h.table.on_frame(msg_frame(&mut it, key, &req), 2_000);
        let [Action::Deliver {
            key: k,
            peer,
            device,
            envelope,
        }] = actions.as_slice()
        else {
            panic!("{actions:?}");
        };
        assert_eq!(*k, key);
        assert_eq!(peer, &dev.fp());
        assert_eq!(device.kind, DeviceKind::Web);
        assert_eq!(envelope, &req);
        assert_eq!(h.table.live_channels(&dev.fp()), vec![key]);
    }

    #[test]
    fn the_state_message_advertises_the_capabilities_of_the_toggles() {
        let h = h();
        let flags = PolicyFlags {
            remote_git_push: true,
            ..PolicyFlags::default()
        };
        let pending = Mode::Pending { since: 0 };
        assert_eq!(
            h.table.state_message_with(&pending, &flags)["capabilities"],
            json!(["cloud", "gitPush"])
        );
        assert_eq!(
            h.table
                .state_message_with(&pending, &PolicyFlags::default())["capabilities"],
            json!(["cloud"])
        );
    }

    #[test]
    fn chunked_one_mib_request_reassembles() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        let req = json!({"rid": 1, "pad": "x".repeat(1 << 20)});
        let bytes = serde_json::to_vec(&req).unwrap();
        let parts = split_message(&bytes, 5, MAX_REQUEST_BYTES).unwrap();
        assert!(parts.len() > 1);
        let mut last = Vec::new();
        for p in parts {
            last = h.table.on_frame(
                frame(FrameType::Transport, key, it.encrypt(&p).unwrap()),
                10,
            );
        }
        let [Action::Deliver { envelope, .. }] = last.as_slice() else {
            panic!("{last:?}");
        };
        assert_eq!(envelope, &req);
    }

    #[test]
    fn fifth_concurrent_chunked_message_gets_bye() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        for id in 0..4 {
            let a = h
                .table
                .on_frame(chunk_frame(&mut it, key, id, 0, false, b"{"), 1);
            assert!(a.is_empty());
        }
        let a = h
            .table
            .on_frame(chunk_frame(&mut it, key, 4, 0, false, b"{"), 1);
        assert_closed_with_bye(&a, key);
        assert!(h.table.is_empty());
    }

    #[test]
    fn stale_reassembly_gets_bye_on_tick() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        // Keep the session active so only the reassembly can expire.
        assert!(h
            .table
            .on_frame(chunk_frame(&mut it, key, 1, 0, false, b"{"), 0)
            .is_empty());
        assert!(h.table.tick(60_000).is_empty());
        assert_closed_with_bye(&h.table.tick(60_001), key);
    }

    #[test]
    fn oversized_reassembly_gets_bye() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        let big = vec![b'a'; MAX_REQUEST_BYTES + 1024];
        let parts = split_message(&big, 1, MAX_REQUEST_BYTES * 2).unwrap();
        let mut closed = None;
        for p in parts {
            let a = h
                .table
                .on_frame(frame(FrameType::Transport, key, it.encrypt(&p).unwrap()), 1);
            if !a.is_empty() {
                closed = Some(a);
                break;
            }
        }
        assert_closed_with_bye(&closed.expect("closed before the end"), key);
    }

    #[test]
    fn unknown_fingerprint_gets_exactly_one_bye() {
        let mut h = h();
        let dev = Dev::new(1);
        let mut init = Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 3).unwrap();
        let hs1 = init.write_hs1(br#"{"name":"x"}"#).unwrap();
        let key = dev.key(3);
        let a = h.table.on_frame(frame(FrameType::Hs1, key, hs1), 0);
        assert!(is_single_bye(&a, key), "{a:?}");
        assert!(h.table.is_empty());
    }

    #[test]
    fn phone_row_and_server_id_mismatch_get_bye() {
        for device in [
            device_row("x", DeviceKind::Phone, None),
            device_row(
                "x",
                DeviceKind::Web,
                Some("00000000-0000-0000-0000-000000000000"),
            ),
        ] {
            let mut h = h();
            let dev = Dev::new(1);
            h.allow(&dev, Lookup::Confirmed { device });
            let mut init =
                Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 1).unwrap();
            let hs1 = init.write_hs1(br#"{"name":"x"}"#).unwrap();
            let a = h.table.on_frame(frame(FrameType::Hs1, dev.key(1), hs1), 0);
            assert!(is_single_bye(&a, dev.key(1)), "{a:?}");
        }
    }

    #[test]
    fn bad_hs1_payload_gets_bye() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        for payload in [&b"not json"[..], br#"{"name":5}"#, br#"{"ua":"x"}"#] {
            let mut init =
                Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 1).unwrap();
            let hs1 = init.write_hs1(payload).unwrap();
            let a = h.table.on_frame(frame(FrameType::Hs1, dev.key(1), hs1), 0);
            assert!(is_single_bye(&a, dev.key(1)), "{a:?}");
        }
        let a = h
            .table
            .on_frame(frame(FrameType::Hs1, dev.key(1), vec![1, 2, 3]), 0);
        assert!(is_single_bye(&a, dev.key(1)));
    }

    #[test]
    fn pending_gets_state_ignores_requests_and_times_out() {
        let mut h = h();
        let dev = Dev::new(1);
        h.allow(&dev, Lookup::Pending);
        let key = dev.key(2);
        let long_name = "n".repeat(200);
        let mut init = Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 2).unwrap();
        let hs1 = init
            .write_hs1(json!({"name": long_name}).to_string().as_bytes())
            .unwrap();
        let actions = h.table.on_frame(frame(FrameType::Hs1, key, hs1), 100);
        let frames = sent(&actions);
        assert_eq!(frames[0].kind, FrameType::Hs2);
        let (_, mut it) = init.read_hs2(&frames[0].body).unwrap();
        let plain = it.decrypt(&frames[1].body).unwrap();
        let Plaintext::Msg(m) = Plaintext::decode(&plain).unwrap() else {
            panic!()
        };
        let state: Value = serde_json::from_slice(&m).unwrap();
        assert_eq!(state["state"], "pending");
        assert_eq!(state["proto"], 1);
        assert!(state.get("permission").is_none());
        let Some(Action::Pending { peer, name, .. }) = actions.last() else {
            panic!("{actions:?}");
        };
        assert_eq!(peer, &dev.fp());
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);

        let req = json!({"version": 1, "rid": 1, "id": "a", "method": "workspace.list"});
        assert!(h
            .table
            .on_frame(msg_frame(&mut it, key, &req), 200)
            .is_empty());
        assert_eq!(h.table.len(), 1);
        assert!(h.table.tick(100 + PENDING_TIMEOUT_MS - 1).is_empty());
        assert_closed_with_bye(&h.table.tick(100 + PENDING_TIMEOUT_MS), key);
    }

    #[test]
    fn a_confirmed_web_session_writes_connect_and_disconnect_lines() {
        let log = std::env::temp_dir().join(format!("monoptah-web-audit-{}", uuid::Uuid::new_v4()));
        let _turn = crate::mobile::audit::test_log(&log);
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let mut init = Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 1).unwrap();
        let hs1 = init.write_hs1(br#"{"name":"x"}"#).unwrap();
        let _ = h.table.on_frame(frame(FrameType::Hs1, key, hs1), 0);
        let events = |fp: &str| -> Vec<String> {
            let mut lines = crate::mobile::audit::read_recent(&log, 50);
            lines.reverse();
            lines
                .iter()
                .filter(|line| line["fingerprint"] == fp[..8] && line["transport"] == "web")
                .map(|line| line["event"].as_str().unwrap_or_default().to_owned())
                .collect()
        };
        assert_eq!(events(&dev.fp()), ["connect"]);
        let _ = h.table.on_frame(frame(FrameType::Bye, key, Vec::new()), 5);
        assert_eq!(events(&dev.fp()), ["connect", "disconnect"]);
        drop(_turn);
        let _ = std::fs::remove_dir_all(&log);
    }

    #[test]
    fn confirm_moves_pending_to_ready() {
        let mut h = h();
        let dev = Dev::new(1);
        h.allow(&dev, Lookup::Pending);
        let key = dev.key(2);
        let mut init = Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 2).unwrap();
        let hs1 = init.write_hs1(br#"{"name":"x"}"#).unwrap();
        let actions = h.table.on_frame(frame(FrameType::Hs1, key, hs1), 0);
        let (_, mut it) = init.read_hs2(&sent(&actions)[0].body).unwrap();
        let _ = it.decrypt(&sent(&actions)[1].body).unwrap();
        let device = device_row(&dev.fp(), DeviceKind::Web, None);
        let a = h.table.confirm(key, device.clone(), 10);
        let frames = sent(&a);
        assert_eq!(frames.len(), 1);
        let plain = it.decrypt(&frames[0].body).unwrap();
        let Plaintext::Msg(m) = Plaintext::decode(&plain).unwrap() else {
            panic!()
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&m).unwrap()["state"],
            "confirmed"
        );
        // Confirming again does nothing; requests are now delivered.
        assert!(h.table.confirm(key, device, 11).is_empty());
        let req = json!({"rid": 1});
        let a = h.table.on_frame(msg_frame(&mut it, key, &req), 12);
        assert!(matches!(a.as_slice(), [Action::Deliver { .. }]));
        // And the pending timeout no longer applies.
        assert!(h.table.tick(PENDING_TIMEOUT_MS + 100).is_empty());
    }

    fn read_envelope(it: &mut Transport, frames: &[Frame]) -> (Value, usize) {
        let mut re = Reassembler::default();
        let mut rekeys = 0;
        for f in frames {
            let plain = it.decrypt(&f.body).unwrap();
            match re.push(&plain, 0).unwrap() {
                Pushed::Rekey => {
                    rekeys += 1;
                    it.rekey_incoming();
                }
                Pushed::Partial => {}
                Pushed::Message(b) => return (serde_json::from_slice(&b).unwrap(), rekeys),
            }
        }
        panic!("no complete message");
    }

    #[test]
    fn desktop_send_splits_large_envelope() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        let env = json!({"rid": 1, "ok": true, "result": {"pad": "y".repeat(200 * 1024)}});
        let frames = h.table.send(key, &env, 5).unwrap();
        assert!(frames.len() > 1);
        assert!(frames.iter().all(|f| f.body.len() <= 65_535));
        let (got, rekeys) = read_envelope(&mut it, &frames);
        assert_eq!(got, env);
        assert_eq!(rekeys, 0);
        let small = json!({"rid": 2});
        let frames = h.table.send(key, &small, 6).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(read_envelope(&mut it, &frames).0, small);
    }

    #[test]
    fn send_errors() {
        let mut h = h();
        let dev = Dev::new(1);
        assert_eq!(
            h.table.send(dev.key(1), &json!({}), 0),
            Err(SendError::UnknownSession)
        );
        h.allow(&dev, Lookup::Pending);
        let mut init = Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 1).unwrap();
        let hs1 = init.write_hs1(br#"{"name":"x"}"#).unwrap();
        h.table.on_frame(frame(FrameType::Hs1, dev.key(1), hs1), 0);
        assert_eq!(
            h.table.send(dev.key(1), &json!({}), 0),
            Err(SendError::NotReady)
        );
    }

    #[test]
    fn outgoing_rekey_on_count_and_time_interoperates() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        h.table.set_rekey_after_messages(3);
        let mut total_rekeys = 0;
        for i in 0..6 {
            let env = json!({"rid": i});
            let frames = h.table.send(key, &env, 10).unwrap();
            let (got, rekeys) = read_envelope(&mut it, &frames);
            assert_eq!(got, env);
            total_rekeys += rekeys;
        }
        assert_eq!(total_rekeys, 2);
        // Time based: one hour after the last rekey.
        let env = json!({"rid": 99});
        let frames = h.table.send(key, &env, 10 + REKEY_AFTER_MS).unwrap();
        let (got, rekeys) = read_envelope(&mut it, &frames);
        assert_eq!((got, rekeys), (env, 1));
    }

    #[test]
    fn incoming_rekey_works() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        let rekey = Plaintext::Rekey.encode().unwrap();
        let f = frame(FrameType::Transport, key, it.encrypt(&rekey).unwrap());
        it.rekey_outgoing();
        assert!(h.table.on_frame(f, 1).is_empty());
        let req = json!({"rid": 3});
        let a = h.table.on_frame(msg_frame(&mut it, key, &req), 2);
        assert!(matches!(a.as_slice(), [Action::Deliver { .. }]));
    }

    #[test]
    fn decryption_failure_and_bad_json_get_bye() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        let mut f = msg_frame(&mut it, key, &json!({"rid": 1}));
        f.body[0] ^= 1;
        assert_closed_with_bye(&h.table.on_frame(f, 1), key);

        let (mut it, _, _) = h.connect(&dev, 1, 2);
        let plain = Plaintext::Msg(b"{nope".to_vec()).encode().unwrap();
        let f = frame(FrameType::Transport, key, it.encrypt(&plain).unwrap());
        assert_closed_with_bye(&h.table.on_frame(f, 3), key);
    }

    #[test]
    fn idle_twelve_hours_gets_bye() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        assert!(h.table.tick(IDLE_LIMIT_MS - 1).is_empty());
        assert_closed_with_bye(&h.table.tick(IDLE_LIMIT_MS), key);

        // A frame in either direction resets the idle clock.
        let (_, _, _) = h.connect(&dev, 1, 100);
        h.table.send(key, &json!({"rid": 1}), 5_000).unwrap();
        assert!(h.table.tick(100 + IDLE_LIMIT_MS).is_empty());
        assert_closed_with_bye(&h.table.tick(5_000 + IDLE_LIMIT_MS), key);
        let _ = &mut it;
    }

    #[test]
    fn idle_session_frame_gets_bye() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        let f = msg_frame(&mut it, key, &json!({"rid": 1}));
        assert_closed_with_bye(&h.table.on_frame(f, IDLE_LIMIT_MS), key);
    }

    #[test]
    fn non_hs1_frames_for_unknown_key_get_bye() {
        let mut h = h();
        let key = SessionKey {
            device_id: [4; 16],
            channel: 1,
        };
        for kind in [FrameType::Transport, FrameType::Hs2] {
            let a = h.table.on_frame(frame(kind, key, vec![0; 40]), 0);
            assert!(is_single_bye(&a, key), "{a:?}");
        }
        // BYE from a device for an unknown key and CTRL/ERROR give nothing.
        assert!(h
            .table
            .on_frame(frame(FrameType::Bye, key, vec![]), 0)
            .is_empty());
        assert!(h
            .table
            .on_frame(frame(FrameType::Ctrl, key, b"{}".to_vec()), 0)
            .is_empty());
    }

    #[test]
    fn device_bye_drops_session_without_reply() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let _ = h.connect(&dev, 1, 0);
        let a = h.table.on_frame(frame(FrameType::Bye, key, vec![]), 1);
        assert_eq!(a, vec![Action::Closed { key }]);
        assert!(h.table.is_empty());
    }

    #[test]
    fn hs2_from_relay_drops_session() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let _ = h.connect(&dev, 1, 0);
        let a = h.table.on_frame(frame(FrameType::Hs2, key, vec![1; 48]), 1);
        assert_closed_with_bye(&a, key);
    }

    #[test]
    fn new_hs1_replaces_old_session() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let _ = h.connect(&dev, 1, 0);
        let (mut it, _, actions) = h.connect(&dev, 1, 5);
        assert_eq!(actions[0], Action::Closed { key });
        assert_eq!(h.table.len(), 1);
        let a = h
            .table
            .on_frame(msg_frame(&mut it, key, &json!({"rid": 1})), 6);
        assert!(matches!(a.as_slice(), [Action::Deliver { .. }]));
    }

    #[test]
    fn per_device_channel_cap() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        for ch in 0..MAX_CHANNELS_PER_DEVICE as u16 {
            let _ = h.connect(&dev, ch, 0);
        }
        let mut init = Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 9).unwrap();
        let hs1 = init.write_hs1(br#"{"name":"x"}"#).unwrap();
        let a = h.table.on_frame(frame(FrameType::Hs1, dev.key(9), hs1), 0);
        assert!(is_single_bye(&a, dev.key(9)), "{a:?}");
        assert_eq!(h.table.len(), MAX_CHANNELS_PER_DEVICE);
        // Re-handshaking an existing channel is still allowed.
        let _ = h.connect(&dev, 0, 1);
    }

    #[test]
    fn total_session_cap() {
        let mut h = h();
        let mut devs = Vec::new();
        for n in 0..(MAX_SESSIONS / MAX_CHANNELS_PER_DEVICE + 1) {
            let dev = Dev::new(n as u8 + 1);
            h.confirmed(&dev);
            devs.push(dev);
        }
        for dev in &devs[..16] {
            for ch in 0..MAX_CHANNELS_PER_DEVICE as u16 {
                let _ = h.connect(dev, ch, 0);
            }
        }
        assert_eq!(h.table.len(), MAX_SESSIONS);
        let dev = &devs[16];
        let mut init = Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 0).unwrap();
        let hs1 = init.write_hs1(br#"{"name":"x"}"#).unwrap();
        let a = h.table.on_frame(frame(FrameType::Hs1, dev.key(0), hs1), 0);
        assert!(is_single_bye(&a, dev.key(0)), "{a:?}");
    }

    #[test]
    fn close_device_and_close_all_and_live_channels() {
        let mut h = h();
        let a = Dev::new(1);
        let b = Dev::new(2);
        h.confirmed(&a);
        h.confirmed(&b);
        let _ = h.connect(&a, 1, 0);
        let _ = h.connect(&a, 2, 0);
        let _ = h.connect(&b, 1, 0);
        assert_eq!(h.table.live_channels(&a.fp()), vec![a.key(1), a.key(2)]);
        let acts = h.table.close_device(&a.id);
        assert_eq!(
            acts,
            vec![
                Action::Send(bye(a.key(1))),
                Action::Closed { key: a.key(1) },
                Action::Send(bye(a.key(2))),
                Action::Closed { key: a.key(2) },
            ]
        );
        assert!(h.table.live_channels(&a.fp()).is_empty());
        assert_eq!(h.table.len(), 1);
        let acts = h.table.close_all();
        assert_closed_with_bye(&acts, b.key(1));
        assert!(h.table.is_empty());
    }

    #[test]
    fn plaintext_limit_is_respected() {
        // Every frame body from send stays within the frame limit even for a
        // message that is an exact multiple of the chunk size.
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        let env = json!({"pad": "z".repeat(MAX_NOISE_PLAINTEXT * 2)});
        let frames = h.table.send(key, &env, 0).unwrap();
        assert!(frames.iter().all(|f| f.encode().is_ok()));
        assert_eq!(read_envelope(&mut it, &frames).0, env);
    }

    #[test]
    fn too_large_send_is_refused() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let _ = h.connect(&dev, 1, 0);
        let env = json!({"pad": "z".repeat(MAX_REQUEST_BYTES + 1)});
        assert_eq!(h.table.send(key, &env, 0), Err(SendError::TooLarge));
        assert_eq!(h.table.len(), 1);
    }

    fn hs1_for(dev: &Dev, h: &H, channel: u16, payload: &[u8]) -> Frame {
        let mut init =
            Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, channel).unwrap();
        frame(
            FrameType::Hs1,
            dev.key(channel),
            init.write_hs1(payload).unwrap(),
        )
    }

    #[test]
    fn a_pending_session_buffers_nothing_and_still_follows_rekeys() {
        let mut h = h();
        let dev = Dev::new(1);
        h.allow(&dev, Lookup::Pending);
        let key = dev.key(2);
        let mut init = Initiator::new(&dev.private, &h.desktop_pub, &DESKTOP, &dev.id, 2).unwrap();
        let hs1 = init.write_hs1(br#"{"name":"x"}"#).unwrap();
        let actions = h.table.on_frame(frame(FrameType::Hs1, key, hs1), 0);
        let (_, mut it) = init.read_hs2(&sent(&actions)[0].body).unwrap();
        let _ = it.decrypt(&sent(&actions)[1].body).unwrap();
        // Four messages opened with non-final chunks, then a fifth: a confirmed
        // session would hold four and close on the fifth. Pending holds none.
        for id in 0..5 {
            let a = h
                .table
                .on_frame(chunk_frame(&mut it, key, id, 0, false, &[b'a'; 4096]), 1);
            assert!(a.is_empty(), "{a:?}");
            assert_eq!(h.table.sessions[&key].reassembler.in_flight(), 0);
        }
        // A rekey still keeps the nonces and keys in step.
        let rekey = Plaintext::Rekey.encode().unwrap();
        let f = frame(FrameType::Transport, key, it.encrypt(&rekey).unwrap());
        it.rekey_outgoing();
        assert!(h.table.on_frame(f, 2).is_empty());
        let a = h
            .table
            .on_frame(msg_frame(&mut it, key, &json!({"rid": 1})), 3);
        assert!(a.is_empty(), "{a:?}");
        assert_eq!(h.table.len(), 1);
        // Garbage that is no plaintext at all still closes the session.
        let f = frame(FrameType::Transport, key, it.encrypt(&[9, 9, 9]).unwrap());
        assert_closed_with_bye(&h.table.on_frame(f, 4), key);
    }

    /// A device that shares `like`'s key but connects under another relay id.
    fn same_key_other_id(like: &Dev, id: u8) -> Dev {
        Dev {
            private: like.private,
            public: like.public,
            id: [id; 16],
        }
    }

    #[test]
    fn pending_sessions_are_capped_per_key_and_in_total() {
        let mut h = h();
        let first = Dev::new(1);
        h.allow(&first, Lookup::Pending);
        for id in 1..=MAX_PENDING_PER_FINGERPRINT as u8 {
            let dev = same_key_other_id(&first, id);
            let a = h
                .table
                .on_frame(hs1_for(&dev, &h, 0, br#"{"name":"x"}"#), 0);
            assert_eq!(sent(&a)[0].kind, FrameType::Hs2, "{a:?}");
        }
        let extra = same_key_other_id(&first, 200);
        let a = h
            .table
            .on_frame(hs1_for(&extra, &h, 0, br#"{"name":"x"}"#), 0);
        assert!(is_single_bye(&a, extra.key(0)), "{a:?}");
        assert_eq!(h.table.len(), MAX_PENDING_PER_FINGERPRINT);
        // Other keys fill the rest of the total, and the next key is refused.
        let mut next = 10u8;
        while h.table.len() < MAX_PENDING_TOTAL {
            let dev = Dev::new(next);
            next += 1;
            h.allow(&dev, Lookup::Pending);
            let a = h
                .table
                .on_frame(hs1_for(&dev, &h, 0, br#"{"name":"x"}"#), 0);
            assert_eq!(sent(&a)[0].kind, FrameType::Hs2, "{a:?}");
        }
        let over = Dev::new(250);
        h.allow(&over, Lookup::Pending);
        let a = h
            .table
            .on_frame(hs1_for(&over, &h, 0, br#"{"name":"x"}"#), 0);
        assert!(is_single_bye(&a, over.key(0)), "{a:?}");
        // A confirmed device is not held back by pending ones.
        let confirmed = Dev::new(251);
        h.confirmed(&confirmed);
        let _ = h.connect(&confirmed, 0, 0);
    }

    #[test]
    fn a_web_row_without_a_server_device_id_gets_bye() {
        let mut h = h();
        let dev = Dev::new(1);
        h.allow(
            &dev,
            Lookup::Confirmed {
                device: device_row(&dev.fp(), DeviceKind::Web, None),
            },
        );
        let a = h
            .table
            .on_frame(hs1_for(&dev, &h, 1, br#"{"name":"x"}"#), 0);
        assert!(is_single_bye(&a, dev.key(1)), "{a:?}");
        assert!(h.table.is_empty());
    }

    #[test]
    fn a_failed_hs1_leaves_the_live_session_intact() {
        let mut h = h();
        let dev = Dev::new(1);
        h.confirmed(&dev);
        let key = dev.key(1);
        let (mut it, _, _) = h.connect(&dev, 1, 0);
        // A device with an unknown key tries the same relay id and channel.
        let stranger = same_key_other_id(&Dev::new(1), 1);
        let a = h
            .table
            .on_frame(hs1_for(&stranger, &h, 1, br#"{"name":"x"}"#), 5);
        // Nothing at all goes out: a BYE would end the live tab's session.
        assert!(a.is_empty(), "{a:?}");
        assert_eq!(h.table.len(), 1);
        let a = h
            .table
            .on_frame(msg_frame(&mut it, key, &json!({"rid": 1})), 6);
        assert!(matches!(a.as_slice(), [Action::Deliver { .. }]), "{a:?}");
    }

    #[test]
    fn untrusted_names_are_cleaned_for_display() {
        assert_eq!(clean_name("  My \t Laptop \n"), "My Laptop");
        assert_eq!(
            clean_name("a\u{202E}b\u{200B}c\u{FEFF}d\u{2066}e\u{2069}f"),
            "abcdef"
        );
        assert_eq!(clean_name("x\u{0}y\u{7f}z\u{85}"), "xyz");
        assert_eq!(clean_name("a    b"), "a b");
        assert_eq!(clean_name(""), UNNAMED_DEVICE);
        assert_eq!(clean_name(" \u{202E}\u{200B} \n "), UNNAMED_DEVICE);
        let long = format!("{} tail", "n".repeat(MAX_NAME_CHARS - 1));
        assert_eq!(clean_name(&long), "n".repeat(MAX_NAME_CHARS - 1));
        assert_eq!(clean_name(&"n".repeat(300)).chars().count(), MAX_NAME_CHARS);
        // And the cleaned name is the one the prompt gets.
        let mut h = h();
        let dev = Dev::new(1);
        h.allow(&dev, Lookup::Pending);
        let hs1 = hs1_for(
            &dev,
            &h,
            1,
            json!({"name": "A\u{202E}B\n"}).to_string().as_bytes(),
        );
        let actions = h.table.on_frame(hs1, 0);
        let Some(Action::Pending { name, .. }) = actions.last() else {
            panic!("{actions:?}");
        };
        assert_eq!(name, "AB");
    }
}
