//! The outbound relay connection of the web link (architecture 4.4 and 4.5).
//!
//! One task dials `wss://<host>/relay/desktop/<id>` with the desktop token,
//! keeps the socket alive with a text `ping`, feeds binary frames to the
//! session table, and reconnects with exponential backoff and full jitter. It
//! binds no port. Nothing here logs frame bytes, plaintext or the token.
use super::frame::{CloseCode, Frame, FrameType, RELAY_ERROR_CODES};
use super::session::{Action, Directory, SendError, SessionKey, SessionTable};
use super::{grants, WebLink, WebState};
use crate::mobile::host::{name as desktop_name, MobileHost};
use crate::mobile::identity_store::WebIdentity;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};
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
/// Requests and watches in flight on one connection.
const MAX_IN_FLIGHT: usize = 64;

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

/// The client task: connect, serve, react to the close, repeat. Returns only
/// when the relay refuses the desktop for good or the identity is unusable.
pub(crate) async fn run(host: MobileHost, link: WebLink, identity: WebIdentity, timing: Timing) {
    let Ok(desktop_uuid) = uuid::Uuid::parse_str(&identity.desktop_id) else {
        link.set(WebState::Off, Some("The web identity is invalid".into()));
        return;
    };
    let url = match relay_url(
        &identity.relay_url,
        &desktop_uuid.hyphenated().to_string(),
        cfg!(debug_assertions),
    ) {
        Ok(url) => url,
        Err(error) => {
            link.set(WebState::Off, Some(error));
            return;
        }
    };
    let token = Zeroizing::new(identity.desktop_token.clone());
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
                    identity.static_private,
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
    let mut bearer = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| End::Lost)?;
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
        Ok(Err(WsError::Http(response))) if response.status().as_u16() == 401 => {
            Err(End::Unauthorized)
        }
        _ => Err(End::Lost),
    }
}

/// A reply to send on a session, from a request task.
struct Out {
    key: SessionKey,
    value: Value,
}

async fn serve_socket(
    socket: Socket,
    host: &MobileHost,
    static_private: [u8; 32],
    desktop_id: [u8; 16],
    timing: &Timing,
) -> End {
    let (mut sink, mut stream) = socket.split();
    let directory_host = host.clone();
    let directory: Directory =
        Box::new(move |fingerprint, _relay_id| directory_host.web_lookup(fingerprint));
    let mut table = SessionTable::new(
        static_private,
        desktop_id,
        desktop_name(),
        env!("CARGO_PKG_VERSION"),
        directory,
    );
    let (out_tx, mut out_rx) = mpsc::channel::<Out>(256);
    let mut requests = Requests::new(host.clone(), out_tx);
    let started = Instant::now();
    let now_ms = move || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut ping = tokio::time::interval_at(started + timing.ping, timing.ping);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut tick = tokio::time::interval_at(started + timing.tick, timing.tick);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut frames: Vec<Frame> = Vec::new();
    let end = loop {
        tokio::select! {
            message = stream.next() => match message {
                None | Some(Err(_)) => break End::Lost,
                Some(Ok(Message::Close(frame))) => {
                    break End::Closed(frame.map(|frame| u16::from(frame.code)));
                }
                Some(Ok(Message::Binary(bytes))) => {
                    on_binary(&bytes, &mut table, &mut requests, now_ms(), &mut frames);
                }
                // Text other than `pong`, and control frames, carry nothing.
                Some(Ok(_)) => {}
            },
            Some(out) = out_rx.recv() => {
                reply(&mut table, &mut requests, out, now_ms(), &mut frames);
            }
            _ = ping.tick() => {
                if sink.send(Message::text("ping")).await.is_err() {
                    break End::Lost;
                }
            }
            _ = tick.tick() => {
                let actions = table.tick(now_ms());
                collect(actions, &mut requests, &mut frames);
            }
        }
        if flush(&mut sink, &mut frames).await.is_err() {
            break End::Lost;
        }
    };
    // The socket is gone: end every session without sending anything.
    let actions = table.close_all();
    collect(actions, &mut requests, &mut Vec::new());
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
fn collect(actions: Vec<Action>, requests: &mut Requests, frames: &mut Vec<Frame>) {
    for action in actions {
        match action {
            Action::Send(frame) => frames.push(frame),
            Action::Deliver {
                key,
                peer,
                envelope,
                ..
            } => requests.deliver(key, peer, envelope),
            Action::Closed { key } => requests.closed(key),
            // The pending approval prompt belongs to the UI (R-15).
            Action::Pending { .. } => {}
        }
    }
}

fn on_binary(
    bytes: &[u8],
    table: &mut SessionTable,
    requests: &mut Requests,
    now: u64,
    frames: &mut Vec<Frame>,
) {
    // A frame that does not decode is dropped.
    let Ok(frame) = Frame::decode(bytes) else {
        return;
    };
    let actions = match frame.kind {
        FrameType::Ctrl => control(&frame.body, table),
        FrameType::Error => {
            relay_error(&frame.body);
            Vec::new()
        }
        _ => table.on_frame(frame, now),
    };
    collect(actions, requests, frames);
}

/// Relay control frames: a device left, or the grants changed.
fn control(body: &[u8], table: &mut SessionTable) -> Vec<Action> {
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
            grants::resync();
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
        }
        Err(SendError::UnknownSession | SendError::NotReady) => {}
    }
}

// --- Request plumbing -------------------------------------------------------
// Minimal per-`rid` serving of the envelopes a session delivers. A shared
// generic serve replaces this section; the host entry points it calls
// (`MobileHost::handle_web` and `watch_web`) stay.

struct Requests {
    host: MobileHost,
    out: mpsc::Sender<Out>,
    tasks: HashMap<(SessionKey, String), JoinHandle<()>>,
}

impl Drop for Requests {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            task.abort();
        }
    }
}

impl Requests {
    fn new(host: MobileHost, out: mpsc::Sender<Out>) -> Self {
        Self {
            host,
            out,
            tasks: HashMap::new(),
        }
    }

    /// The watches and requests of a session end with it.
    fn closed(&mut self, key: SessionKey) {
        self.tasks.retain(|(owner, _), task| {
            let keep = *owner != key;
            if !keep {
                task.abort();
            }
            keep
        });
    }

    fn deliver(&mut self, key: SessionKey, peer: String, mut envelope: Value) {
        let Some(object) = envelope.as_object_mut() else {
            return;
        };
        let Some(rid) = object.remove("rid") else {
            return;
        };
        let id = rid.to_string();
        let cancel = object.remove("cancel").is_some_and(|v| v == true);
        self.tasks.retain(|_, task| !task.is_finished());
        if cancel {
            if let Some(task) = self.tasks.remove(&(key, id)) {
                task.abort();
            }
            return;
        }
        let out = self.out.clone();
        if self.tasks.len() >= MAX_IN_FLIGHT {
            let busy = json!({"rid": rid, "ok": false, "error": "Desktop is busy; try again"});
            let _ = out.try_send(Out { key, value: busy });
            return;
        }
        let watch = envelope["watch"] == true;
        let host = self.host.clone();
        let task = tokio::spawn(async move {
            if watch {
                serve_watch(host, peer, envelope, key, rid, out).await;
            } else {
                let value = match host.handle_web(&peer, envelope).await {
                    Ok(result) => json!({"rid": rid, "ok": true, "result": result}),
                    Err(error) => json!({"rid": rid, "ok": false, "error": error}),
                };
                let _ = out.send(Out { key, value }).await;
            }
        });
        if let Some(old) = self.tasks.insert((key, id), task) {
            old.abort();
        }
    }
}

async fn serve_watch(
    host: MobileHost,
    peer: String,
    request: Value,
    key: SessionKey,
    rid: Value,
    out: mpsc::Sender<Out>,
) {
    let (sink, mut updates) = mpsc::channel::<Value>(32);
    if let Err(error) = host.watch_web(&peer, request, sink).await {
        let value = json!({"rid": rid, "ok": false, "error": error});
        let _ = out.send(Out { key, value }).await;
        return;
    }
    while let Some(update) = updates.recv().await {
        let value = json!({"rid": rid, "ok": true, "result": update});
        if out.send(Out { key, value }).await.is_err() {
            break;
        }
    }
}
