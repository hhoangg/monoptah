//! The outbound relay connection of the web link (architecture 4.4 and 4.5).
//!
//! One task dials `wss://<host>/relay/desktop/<id>` with the desktop token,
//! keeps the socket alive with a text `ping`, feeds binary frames to the
//! session table, and reconnects with exponential backoff and full jitter. It
//! binds no port. Nothing here logs frame bytes, plaintext or the token.
use super::channel::{Out, Requests};
use super::desktop_api;
use super::frame::{CloseCode, Frame, FrameType, RELAY_ERROR_CODES};
use super::session::{Action, Directory, Lookup, SendError, SessionTable, PROTO};
use super::{LinkCommand, WebLink, WebState};
use crate::mobile::host::MobileHost;
use crate::mobile::identity_store::WebIdentity;
use crate::mobile::policy;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::{sync::mpsc, time::Instant};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::{header::AUTHORIZATION, HeaderValue},
    protocol::WebSocketConfig,
    Error as WsError, Message,
};
use zeroize::Zeroizing;

/// What the desktop shows when the relay refused it for good.
pub const REVOKED_TEXT: &str = "Web app: access revoked, pair again";
/// Longest wait for the relay to accept the WebSocket upgrade.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Every duration the client uses, injectable so tests need not wait.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    /// First backoff ceiling.
    pub base: Duration,
    /// Backoff cap.
    pub cap: Duration,
    /// Wait after a rate-limit or room-quota close.
    pub penalty: Duration,
    /// Keepalive `ping` interval.
    pub ping: Duration,
    /// How often the session table's timeouts run.
    pub tick: Duration,
    /// A connection this old resets the backoff and the one-shot retries.
    pub stable: Duration,
    /// No inbound message at all (the relay answers `ping` with `pong`) for
    /// this long means the path is dead: the connection counts as lost.
    pub read_deadline: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            cap: Duration::from_secs(60),
            penalty: Duration::from_secs(60),
            ping: Duration::from_secs(30),
            tick: Duration::from_secs(5),
            stable: Duration::from_secs(30),
            read_deadline: Duration::from_secs(90),
        }
    }
}

/// The relay URL for `host_url` (`https://H[:p]` or `wss://H[:p]`) and the
/// desktop id. `http://` and `ws://` are accepted only when
/// `allow_insecure_localhost` is set and the host is local; production passes
/// `cfg!(debug_assertions)`. A path, query, fragment or userinfo is refused.
pub fn relay_url(
    host_url: &str,
    desktop_id: &str,
    allow_insecure_localhost: bool,
) -> Result<String, String> {
    if desktop_id.is_empty()
        || !desktop_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Invalid desktop id".into());
    }
    let (secure, rest) = if let Some(rest) = host_url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = host_url.strip_prefix("wss://") {
        (true, rest)
    } else if let Some(rest) = host_url.strip_prefix("http://") {
        (false, rest)
    } else if let Some(rest) = host_url.strip_prefix("ws://") {
        (false, rest)
    } else {
        return Err("The web host must be an https:// address".into());
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', '\\', ' ']) {
        return Err("The web host address must be a bare host".into());
    }
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner.split_once(']').ok_or("Invalid web host address")?;
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':').ok_or("Invalid web host address")?),
        };
        (format!("[{host}]"), port)
    } else {
        match rest.split_once(':') {
            Some((host, port)) => (host.to_string(), Some(port)),
            None => (rest.to_string(), None),
        }
    };
    let port = match port {
        None | Some("") => None,
        Some(port) => {
            let valid = port.len() <= 5
                && port.chars().all(|c| c.is_ascii_digit())
                && port.parse::<u32>().is_ok_and(|p| (1..=65_535).contains(&p));
            if !valid {
                return Err("Invalid web host port".into());
            }
            Some(port)
        }
    };
    if host.is_empty() || host == "[]" || (host.contains(':') && !host.starts_with('[')) {
        return Err("Invalid web host address".into());
    }
    if !secure {
        let local = matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
        if !allow_insecure_localhost || !local {
            return Err("The web host must use https://".into());
        }
    }
    let scheme = if secure { "wss" } else { "ws" };
    let authority = match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    Ok(format!("{scheme}://{authority}/relay/desktop/{desktop_id}"))
}

/// The backoff ceiling for attempt `attempt` (0 based): `base`, doubling, capped.
pub(crate) fn backoff_ceiling(timing: &Timing, attempt: u32) -> Duration {
    timing
        .base
        .saturating_mul(1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX))
        .min(timing.cap)
}

/// Full jitter: uniform in `[0, ceiling]`.
fn jitter(ceiling: Duration) -> Duration {
    use rand::Rng;
    let millis = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
    Duration::from_millis(rand::rng().random_range(0..=millis))
}

/// How one connection ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    /// The relay closed with this code (`None`: a close frame with no code).
    Closed(Option<u16>),
    /// The socket failed or ended without a close frame.
    Lost,
    /// The upgrade was refused with HTTP 401.
    Unauthorized,
}

/// What to do after a connection ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Stop { state: WebState, error: String },
    Immediate,
    Backoff { reason: Option<String> },
    Penalty { reason: String },
}

/// The one-shot retries of architecture 4.5 (1001 and 4009).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    going_away: bool,
    replaced: bool,
}

impl Budget {
    pub(crate) fn new() -> Self {
        Self {
            going_away: true,
            replaced: true,
        }
    }
}

/// The close-code table of architecture 4.5, from the desktop's side.
pub(crate) fn react(end: End, budget: &mut Budget) -> Step {
    let revoked = || Step::Stop {
        state: WebState::Revoked,
        error: REVOKED_TEXT.into(),
    };
    let code = match end {
        End::Lost => {
            return Step::Backoff {
                reason: Some("The connection to the relay was lost".into()),
            }
        }
        End::Unauthorized => return revoked(),
        End::Closed(code) => code,
    };
    match code {
        Some(1001) if std::mem::take(&mut budget.going_away) => Step::Immediate,
        Some(4009) if std::mem::take(&mut budget.replaced) => Step::Immediate,
        Some(1008) => Step::Stop {
            state: WebState::Offline,
            error: "The relay refused the connection (policy)".into(),
        },
        Some(1009) => Step::Stop {
            state: WebState::Offline,
            error: "The relay refused a message as too big".into(),
        },
        Some(4001 | 4003 | 4011) => revoked(),
        Some(4008) => Step::Penalty {
            reason: "The relay is rate limiting this desktop".into(),
        },
        Some(4010) => Step::Penalty {
            reason: "The relay has no room for this desktop".into(),
        },
        Some(code) => Step::Backoff {
            reason: CloseCode::from_code(code)
                .filter(|c| *c != CloseCode::Normal)
                .map(|c| format!("The relay closed the connection ({})", c.as_str())),
        },
        None => Step::Backoff { reason: None },
    }
}

/// The WebSocket address to connect to: always the canonical one built from
/// the host and the desktop id (`wss` for https, lowercase hyphenated id). The
/// stored string is used only when it is exactly that address; any other form
/// (an `https` scheme, an upper-case id, a bare host from an older install)
/// falls back to the canonical one. `base` is the checked HTTP base of `stored`.
pub(crate) fn connect_url(
    stored: &str,
    base: &str,
    desktop_uuid: &uuid::Uuid,
) -> Result<String, String> {
    let canonical = relay_url(
        base,
        &desktop_uuid.hyphenated().to_string(),
        cfg!(debug_assertions),
    )?;
    Ok(if stored.trim() == canonical {
        stored.trim().to_owned()
    } else {
        canonical
    })
}

/// The client task: connect, serve, react to the close, repeat. Returns only
/// when the relay refuses the desktop for good or the identity is unusable.
pub(crate) async fn run(host: MobileHost, link: WebLink, identity: WebIdentity, timing: Timing) {
    let Ok(desktop_uuid) = uuid::Uuid::parse_str(&identity.desktop_id) else {
        link.set(WebState::Off, Some("The web identity is invalid".into()));
        return;
    };
    // The stored address is the full `.../relay/desktop/<id>` form the server
    // returned at pairing (an older install stored the bare host).
    let base = match desktop_api::base_url(&identity.relay_url, cfg!(debug_assertions)) {
        Ok(base) => base,
        Err(error) => {
            link.set(WebState::Off, Some(error));
            return;
        }
    };
    let url = match connect_url(&identity.relay_url, &base, &desktop_uuid) {
        Ok(url) => url,
        Err(error) => {
            link.set(WebState::Off, Some(error));
            return;
        }
    };
    let token = Zeroizing::new(identity.desktop_token.clone());
    let sync = GrantSync {
        host: host.clone(),
        link: link.clone(),
        base,
        token: Arc::new(token.clone()),
        generation: link.generation(),
    };
    // Our own copy of the desktop key, wiped when the client ends. snow keeps
    // further copies it does not wipe (see `SessionTable`).
    let static_private = Zeroizing::new(identity.static_private);
    let mut attempt = 0u32;
    let mut budget = Budget::new();
    loop {
        link.set(WebState::Connecting, None);
        let mut connected_for = Duration::ZERO;
        let end = match connect(&url, &token).await {
            Ok(socket) => {
                link.set(WebState::Connected, None);
                let since = Instant::now();
                let end = serve_socket(
                    socket,
                    &host,
                    &link,
                    &sync,
                    &static_private,
                    *desktop_uuid.as_bytes(),
                    &timing,
                )
                .await;
                connected_for = since.elapsed();
                end
            }
            Err(end) => end,
        };
        if connected_for >= timing.stable {
            attempt = 0;
            budget = Budget::new();
        }
        let wait = match react(end, &mut budget) {
            Step::Stop { state, error } => {
                link.set(state, Some(error));
                return;
            }
            Step::Immediate => {
                link.set(WebState::Offline, None);
                Duration::ZERO
            }
            Step::Backoff { reason } => {
                link.set(WebState::Offline, reason);
                let wait = jitter(backoff_ceiling(&timing, attempt));
                attempt = attempt.saturating_add(1);
                wait
            }
            Step::Penalty { reason } => {
                link.set(WebState::Offline, Some(reason));
                timing.penalty
            }
        };
        tokio::time::sleep(wait).await;
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(url: &str, token: &str) -> Result<Socket, End> {
    let mut request = url.into_client_request().map_err(|_| End::Lost)?;
    // The header text is wiped once the value is built. The `HeaderValue`'s
    // own bytes (and the request that carries them) are not wipeable.
    let header = Zeroizing::new(format!("Bearer {token}"));
    let mut bearer = HeaderValue::from_str(&header).map_err(|_| End::Lost)?;
    bearer.set_sensitive(true);
    request.headers_mut().insert(AUTHORIZATION, bearer);
    let config = WebSocketConfig::default()
        .max_message_size(Some(super::frame::MAX_FRAME_SIZE * 2))
        .max_frame_size(Some(super::frame::MAX_FRAME_SIZE * 2));
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await
    {
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(WsError::Http(response))) => Err(handshake_end(response.status().as_u16())),
        _ => Err(End::Lost),
    }
}

/// How a refused WebSocket handshake ends the attempt: 401 is a revoked
/// token, 429 is the relay's `slow_down` (the same wait as close 4008), any
/// other status is a lost connection to retry with backoff.
pub(crate) fn handshake_end(status: u16) -> End {
    match status {
        401 => End::Unauthorized,
        429 => End::Closed(Some(4008)),
        _ => End::Lost,
    }
}

/// Fetches `GET /api/desktop/grants` and applies it, off the async workers.
/// One sync runs at a time; a request that arrives meanwhile repeats it.
#[derive(Clone)]
struct GrantSync {
    host: MobileHost,
    link: WebLink,
    base: String,
    token: Arc<Zeroizing<String>>,
    /// The link generation this client belongs to: a sync that finishes after
    /// an unpair or a restart applies nothing.
    generation: u64,
}

impl GrantSync {
    fn trigger(&self) {
        if !self.link.sync_gate().enter() {
            return;
        }
        let this = self.clone();
        tokio::task::spawn_blocking(move || loop {
            this.once();
            if !this.link.sync_gate().again_or_leave() {
                break;
            }
        });
    }

    fn once(&self) {
        let Ok((grants, unreadable)) = desktop_api::desktop_grants(&self.base, &self.token) else {
            return;
        };
        for (device_id, state, permission) in
            self.host
                .apply_server_grants_listed(&grants, &unreadable, self.generation)
        {
            // Best effort: the next sync repeats what the server still lacks.
            let answer = desktop_api::report_grant_state(
                &self.base,
                &self.token,
                &device_id,
                state,
                permission,
            );
            // The server says the grant is gone or revoked: go round once
            // more, which drops the row it still has here.
            if answer == Ok(desktop_api::Reported::Gone) {
                self.link.sync_gate().enter();
            }
        }
    }
}

async fn serve_socket(
    socket: Socket,
    host: &MobileHost,
    link: &WebLink,
    sync: &GrantSync,
    static_private: &Zeroizing<[u8; 32]>,
    desktop_id: [u8; 16],
    timing: &Timing,
) -> End {
    let (mut sink, mut stream) = socket.split();
    let directory_host = host.clone();
    let directory: Directory =
        Box::new(move |fingerprint, relay_id| directory_host.web_lookup_for(fingerprint, relay_id));
    let mut table = SessionTable::new(
        static_private.clone(),
        desktop_id,
        link.desktop_name(),
        env!("CARGO_PKG_VERSION"),
        directory,
    );
    let (out_tx, mut out_rx) = mpsc::channel::<Out>(256);
    let mut requests = Requests::new(host.clone(), out_tx);
    let mut commands = link.install_commands(sync.generation);
    // Every connect fetches the grants: the pending ones live in memory only.
    sync.trigger();
    let started = Instant::now();
    let now_ms = move || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut ping = tokio::time::interval_at(started + timing.ping, timing.ping);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut tick = tokio::time::interval_at(started + timing.tick, timing.tick);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut frames: Vec<Frame> = Vec::new();
    let mut last_read = Instant::now();
    let end = loop {
        tokio::select! {
            // An inbound message that is ready wins over the deadline, so a
            // long outbound flush with pongs buffered is not taken for a
            // dead link.
            biased;
            message = stream.next() => match message {
                None | Some(Err(_)) => break End::Lost,
                Some(Ok(Message::Close(frame))) => {
                    break End::Closed(frame.map(|frame| u16::from(frame.code)));
                }
                Some(Ok(Message::Binary(bytes))) => {
                    last_read = Instant::now();
                    on_binary(&bytes, &mut table, &mut requests, link, sync, now_ms(), &mut frames);
                }
                // Text other than `pong`, and control frames, carry nothing
                // but they prove the relay is there.
                Some(Ok(_)) => last_read = Instant::now(),
            },
            // Nothing at all from the relay, not even a `pong`: the path is
            // dead (a sleeping laptop, a dropped NAT mapping).
            _ = tokio::time::sleep_until(last_read + timing.read_deadline) => break End::Lost,
            Some(out) = out_rx.recv() => {
                reply(&mut table, &mut requests, out, now_ms(), &mut frames);
            }
            Some(command) = commands.recv() => {
                if matches!(command, LinkCommand::SyncGrants) {
                    sync.trigger();
                } else {
                    run_command(command, host, link, &mut table, &mut requests, now_ms(), &mut frames);
                }
            }
            _ = ping.tick() => {
                if sink.send(Message::text("ping")).await.is_err() {
                    break End::Lost;
                }
            }
            _ = tick.tick() => {
                let actions = table.tick(now_ms());
                collect(actions, &mut requests, link, &mut frames);
            }
        }
        if flush(&mut sink, &mut frames).await.is_err() {
            break End::Lost;
        }
    };
    // The socket is gone: end every session without sending anything.
    let actions = table.close_all();
    collect(actions, &mut requests, link, &mut Vec::new());
    link.end_run(sync.generation);
    end
}

async fn flush<S>(sink: &mut S, frames: &mut Vec<Frame>) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    for frame in frames.drain(..) {
        let Ok(bytes) = frame.encode() else { continue };
        sink.send(Message::Binary(bytes.into()))
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

/// Turns the table's actions into frames to send and requests to serve.
fn collect(actions: Vec<Action>, requests: &mut Requests, link: &WebLink, frames: &mut Vec<Frame>) {
    for action in actions {
        match action {
            Action::Send(frame) => frames.push(frame),
            Action::Deliver {
                key,
                peer,
                envelope,
                ..
            } => requests.deliver(key, peer, envelope),
            Action::Closed { key } => {
                link.live_remove(&key);
                requests.closed(key);
            }
            // Settings lists the device as online while its session is open.
            Action::Pending { key, peer, .. } => link.live_insert(key, peer),
        }
    }
}

fn on_binary(
    bytes: &[u8],
    table: &mut SessionTable,
    requests: &mut Requests,
    link: &WebLink,
    sync: &GrantSync,
    now: u64,
    frames: &mut Vec<Frame>,
) {
    // A frame that does not decode is dropped.
    let Ok(frame) = Frame::decode(bytes) else {
        return;
    };
    let actions = match frame.kind {
        FrameType::Ctrl => control(&frame.body, table, sync),
        FrameType::Error => {
            relay_error(&frame.body);
            Vec::new()
        }
        _ => table.on_frame(frame, now),
    };
    collect(actions, requests, link, frames);
}

/// Relay control frames: a device left, or the grants changed.
fn control(body: &[u8], table: &mut SessionTable, sync: &GrantSync) -> Vec<Action> {
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    match message["t"].as_str() {
        Some("device") if message["event"] == "left" => message["id"]
            .as_str()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .map(|id| table.close_device(id.as_bytes()))
            .unwrap_or_default(),
        Some("grants") => {
            sync.trigger();
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Logs a relay ERROR frame by its code only.
fn relay_error(body: &[u8]) {
    let code = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value["code"].as_str().map(str::to_owned));
    let known = code
        .as_deref()
        .and_then(|code| RELAY_ERROR_CODES.iter().find(|known| **known == code));
    eprintln!(
        "web link: relay error {}",
        known.copied().unwrap_or("unknown")
    );
}

fn reply(
    table: &mut SessionTable,
    requests: &mut Requests,
    out: Out,
    now: u64,
    frames: &mut Vec<Frame>,
) {
    // A watch ended for a frame that was too large sends nothing more.
    if requests.is_ended(out.key, &out.value["rid"]) {
        return;
    }
    match table.send(out.key, &out.value, now) {
        Ok(sent) => frames.extend(sent),
        Err(SendError::Encrypt) => {
            frames.push(SessionTable::bye_frame(out.key));
            requests.closed(out.key);
        }
        Err(SendError::TooLarge) => {
            let rid = out.value["rid"].clone();
            let error = json!({"rid": rid, "ok": false, "error": "Response too large"});
            if let Ok(sent) = table.send(out.key, &error, now) {
                frames.extend(sent);
            }
            // A watch whose frame cannot be sent ends: the frames only grow.
            // The error above tells the browser, which reopens with `from`.
            requests.cancel(out.key, &rid);
        }
        Err(SendError::UnknownSession | SendError::NotReady) => {}
    }
}

/// The `state` message of a confirmed session, as `SessionTable` sends it.
fn confirmed_state(permission: policy::Permission, name: &str) -> Value {
    json!({
        "type": "state",
        "state": "confirmed",
        "permission": desktop_api::permission_name(permission),
        "proto": PROTO,
        "capabilities": policy::capabilities(&policy::flags()),
        "desktop": { "name": name, "version": env!("CARGO_PKG_VERSION") },
    })
}

/// Ends every session of the relay devices behind `fingerprint` with a BYE,
/// after telling each ready one `state: revoked` so the browser shows "Access
/// revoked" and wipes its cache even when the server report is late.
fn close_fingerprint(
    fingerprint: &str,
    table: &mut SessionTable,
    requests: &mut Requests,
    link: &WebLink,
    now: u64,
    frames: &mut Vec<Frame>,
) {
    let revoked = json!({"type": "state", "state": "revoked"});
    for key in table.live_channels(fingerprint) {
        // A session that is not ready (pending) cannot carry it; it gets BYE only.
        if let Ok(sent) = table.send(key, &revoked, now) {
            frames.extend(sent);
        }
    }
    let mut devices: Vec<[u8; 16]> = table
        .live_channels(fingerprint)
        .iter()
        .map(|key| key.device_id)
        .collect();
    devices.dedup();
    for device in devices {
        let actions = table.close_device(&device);
        collect(actions, requests, link, frames);
    }
}

/// A desktop action on the live sessions (confirm, deny, revoke, resend).
fn run_command(
    command: LinkCommand,
    host: &MobileHost,
    link: &WebLink,
    table: &mut SessionTable,
    requests: &mut Requests,
    now: u64,
    frames: &mut Vec<Frame>,
) {
    match command {
        LinkCommand::Confirm {
            fingerprint,
            device,
        } => {
            // Only a session under the relay id the grant named becomes ready:
            // any other of this key is ended.
            let bound = device.server_device_id.clone().unwrap_or_default();
            for key in table.live_channels(&fingerprint) {
                link.live_remove(&key);
                let id = uuid::Uuid::from_bytes(key.device_id)
                    .hyphenated()
                    .to_string();
                let actions = if id.eq_ignore_ascii_case(&bound) {
                    table.confirm(key, (*device).clone(), now)
                } else {
                    table.close_device(&key.device_id)
                };
                collect(actions, requests, link, frames);
            }
        }
        LinkCommand::Deny { fingerprint } | LinkCommand::Revoke { fingerprint } => {
            close_fingerprint(&fingerprint, table, requests, link, now, frames);
        }
        // Handled by the loop, which owns the grant sync.
        LinkCommand::SyncGrants => {}
        LinkCommand::ResendAll => {
            // The permission comes from the desktop's current row, as for
            // `Resend`, never from the copy stored when the tab connected.
            for (key, fingerprint) in table.ready_sessions() {
                let Lookup::Confirmed { device } = host.web_lookup(&fingerprint) else {
                    continue;
                };
                match table.send(
                    key,
                    &confirmed_state(device.permission, &link.desktop_name()),
                    now,
                ) {
                    Ok(sent) => frames.extend(sent),
                    Err(SendError::Encrypt) => {
                        frames.push(SessionTable::bye_frame(key));
                        requests.closed(key);
                    }
                    Err(_) => {}
                }
            }
        }
        LinkCommand::Resend { fingerprint } => {
            let Lookup::Confirmed { device } = host.web_lookup(&fingerprint) else {
                return;
            };
            let state = confirmed_state(device.permission, &link.desktop_name());
            for key in table.live_channels(&fingerprint) {
                match table.send(key, &state, now) {
                    Ok(sent) => frames.extend(sent),
                    Err(SendError::Encrypt) => {
                        frames.push(SessionTable::bye_frame(key));
                        requests.closed(key);
                    }
                    Err(_) => {}
                }
            }
        }
    }
}
