//! Tests of the relay client against a local stub relay: a tokio-tungstenite
//! server on 127.0.0.1 plays the relay, and the Noise `Initiator` plays the
//! browser. Nothing leaves localhost.
use super::channel::{Out, Requests};
use super::frame::{Frame, FrameType, Plaintext};
use super::noise::{fingerprint, generate_static, Initiator, Transport};
use super::relay::{backoff_ceiling, react, relay_url, Budget, End, Step, Timing, REVOKED_TEXT};
use super::session::{Lookup, SessionKey, SessionTable};
use super::{WebState, WebStatus};
use crate::mobile::host::{Device, Event, MobileHost, Push};
use crate::mobile::identity_store::WebIdentity;
use crate::mobile::policy::{DeviceKind, Permission};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{future::Future, time::Duration};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::{
    protocol::{frame::coding::CloseCode as WsCode, CloseFrame},
    Message,
};
use tokio_tungstenite::WebSocketStream;

const TOKEN: &str = "test-token-not-real";
const DESKTOP_UUID: &str = "11111111-2222-4333-8444-555555555555";
const WAIT: Duration = Duration::from_secs(8);

fn fast() -> Timing {
    Timing {
        base: Duration::from_millis(40),
        cap: Duration::from_millis(200),
        penalty: Duration::from_millis(200),
        ping: Duration::from_millis(60),
        tick: Duration::from_millis(100),
        stable: Duration::from_secs(30),
        read_deadline: Duration::from_secs(30),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(WAIT, f).await.expect("timed out")
}

/// The stub relay: accepts desktop sockets and hands each to the test.
struct Stub {
    listener: TcpListener,
}

struct Conn {
    ws: WebSocketStream<TcpStream>,
    path: String,
    auth: String,
    pings: usize,
}

impl Stub {
    async fn bind(port: u16) -> Self {
        let mut tries = 0;
        loop {
            match TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => return Self { listener },
                Err(_) if tries < 50 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
                Err(error) => panic!("bind failed: {error}"),
            }
        }
    }

    fn port(&self) -> u16 {
        self.listener.local_addr().unwrap().port()
    }

    #[allow(clippy::result_large_err)] // the handshake callback's fixed signature
    async fn accept(&self) -> Conn {
        // The desktop also calls the web server's HTTP API on this port (grant
        // sync). This stub plays the relay only: answer those with 404.
        let tcp = loop {
            let (mut tcp, _) = self.listener.accept().await.unwrap();
            let mut head = [0u8; 16];
            let mut seen = 0;
            for _ in 0..40 {
                seen = tcp.peek(&mut head).await.unwrap_or(0);
                if seen >= 11 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            if head[..seen].starts_with(b"GET /relay/") {
                break tcp;
            }
            use tokio::io::AsyncWriteExt;
            let _ = tcp
                .write_all(b"HTTP/1.1 404 X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        };
        let mut seen = (String::new(), String::new());
        let ws = tokio_tungstenite::accept_hdr_async(
            tcp,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                seen.0 = request.uri().path().to_string();
                seen.1 = request
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                Ok(response)
            },
        )
        .await
        .unwrap();
        Conn {
            ws,
            path: seen.0,
            auth: seen.1,
            pings: 0,
        }
    }
}

impl Conn {
    /// The next binary frame the desktop sent; counts `ping` texts on the way.
    async fn frame(&mut self) -> Frame {
        within(async {
            loop {
                match self.ws.next().await.expect("socket ended").unwrap() {
                    Message::Binary(bytes) => return Frame::decode(&bytes).unwrap(),
                    Message::Text(text) if text.as_str() == "ping" => self.pings += 1,
                    _ => {}
                }
            }
        })
        .await
    }

    async fn send(&mut self, frame: Frame) {
        self.ws
            .send(Message::Binary(frame.encode().unwrap().into()))
            .await
            .unwrap();
    }

    async fn close(&mut self, code: u16) {
        let frame = CloseFrame {
            code: WsCode::from(code),
            reason: "".into(),
        };
        let _ = self.ws.close(Some(frame)).await;
    }

    /// True once the desktop closed or dropped its end.
    async fn ended(&mut self) -> bool {
        within(async {
            loop {
                match self.ws.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return true,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await
    }
}

/// The browser: a Noise initiator with its own keys.
struct Peer {
    private: [u8; 32],
    public: [u8; 32],
    id: [u8; 16],
    desktop_public: [u8; 32],
    transport: Option<Transport>,
    channel: u16,
}

impl Peer {
    fn new(id: u8, desktop_public: [u8; 32]) -> Self {
        let (private, public) = generate_static().unwrap();
        Self {
            private,
            public,
            id: [id; 16],
            desktop_public,
            transport: None,
            channel: 0,
        }
    }

    fn fingerprint(&self) -> String {
        fingerprint(&self.public)
    }

    fn relay_id(&self) -> String {
        uuid::Uuid::from_bytes(self.id).hyphenated().to_string()
    }

    fn frame(&self, kind: FrameType, body: Vec<u8>) -> Frame {
        Frame {
            kind,
            channel: self.channel,
            device_id: self.id,
            body,
        }
    }

    /// Runs the handshake and returns the first state message.
    async fn handshake(&mut self, conn: &mut Conn, channel: u16) -> Value {
        self.channel = channel;
        let desktop = *uuid::Uuid::parse_str(DESKTOP_UUID).unwrap().as_bytes();
        let mut init = Initiator::new(
            &self.private,
            &self.desktop_public,
            &desktop,
            &self.id,
            channel,
        )
        .unwrap();
        let hs1 = init.write_hs1(br#"{"name":"Laptop","ua":"test"}"#).unwrap();
        conn.send(self.frame(FrameType::Hs1, hs1)).await;
        let hs2 = conn.frame().await;
        assert_eq!(hs2.kind, FrameType::Hs2);
        let (_, transport) = init.read_hs2(&hs2.body).unwrap();
        self.transport = Some(transport);
        self.read(conn).await
    }

    async fn read(&mut self, conn: &mut Conn) -> Value {
        let frame = conn.frame().await;
        assert_eq!(frame.kind, FrameType::Transport);
        let plain = self
            .transport
            .as_mut()
            .unwrap()
            .decrypt(&frame.body)
            .unwrap();
        let Plaintext::Msg(bytes) = Plaintext::decode(&plain).unwrap() else {
            panic!("expected a message");
        };
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn write(&mut self, conn: &mut Conn, envelope: &Value) {
        let plain = Plaintext::Msg(serde_json::to_vec(envelope).unwrap())
            .encode()
            .unwrap();
        let body = self.transport.as_mut().unwrap().encrypt(&plain).unwrap();
        conn.send(self.frame(FrameType::Transport, body)).await;
    }

    async fn call(&mut self, conn: &mut Conn, rid: u64, method: &str) -> Value {
        let request = json!({"version": 1, "rid": rid, "id": format!("c{rid}"), "method": method, "params": {}});
        self.write(conn, &request).await;
        self.read(conn).await
    }
}

struct Env {
    host: MobileHost,
    peer: Peer,
    _dir: tempfile::TempDir,
}

fn web_row(peer: &Peer) -> Device {
    Device {
        id: "web-1".into(),
        name: "Laptop".into(),
        fingerprint: peer.fingerprint(),
        push: None,
        kind: DeviceKind::Web,
        permission: Permission::Control,
        projects: None,
        monos: None,
        server_device_id: Some(peer.relay_id()),
        created_at: None,
        last_seen: None,
    }
}

/// A host with a sealed web identity pointing at `http://127.0.0.1:<port>`
/// and one paired web device. Nothing is switched on yet.
fn env(port: u16) -> Env {
    env_with_events(port).0
}

/// `env`, with the app's side of the host: the events a request or watch raises.
fn env_with_events(port: u16) -> (Env, async_channel::Receiver<Event>) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("store.db");
    rusqlite::Connection::open(&database).unwrap();
    crate::accounts::register_database(dir.path(), &database);
    let (host, events) = MobileHost::open(dir.path().join("mobile-access.json")).unwrap();
    let (private, public) = generate_static().unwrap();
    host.save_web_identity(&WebIdentity {
        static_private: private,
        static_public: public,
        desktop_id: DESKTOP_UUID.into(),
        desktop_token: TOKEN.into(),
        relay_url: format!("http://127.0.0.1:{port}"),
        account_handle: None,
    })
    .unwrap();
    let peer = Peer::new(7, public);
    host.add_device_for_test(web_row(&peer));
    (
        Env {
            host,
            peer,
            _dir: dir,
        },
        events,
    )
}

async fn status_is(host: &MobileHost, state: WebState) -> WebStatus {
    within(async {
        loop {
            let status = host.web_status();
            if status.state == state {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

// (a) handshake and a request over the web path while QUIC is off.
#[test]
fn web_request_is_served_while_quic_is_disabled() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let mut env = env(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        assert_eq!(conn.path, format!("/relay/desktop/{DESKTOP_UUID}"));
        let state = env.peer.handshake(&mut conn, 3).await;
        assert_eq!(state["state"], "confirmed");
        let reply = env.peer.call(&mut conn, 1, "workspace.list").await;
        assert_eq!(reply["rid"], 1);
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(reply["result"]["projects"].is_array());
        // A phone-only method that needs a row of another kind is refused.
        let reply = env.peer.call(&mut conn, 2, "device.enroll").await;
        assert_eq!(reply["ok"], false);
    });
    assert!(!env.host.status().enabled);
}

// (b) the same request fails when the web flag is off.
#[test]
fn web_request_is_refused_when_web_is_disabled() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let mut env = env(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        env.peer.handshake(&mut conn, 1).await;
        // QUIC on, web off, in memory: the client keeps running.
        env.host.set_flags_for_test(true, false);
        let reply = env.peer.call(&mut conn, 1, "workspace.list").await;
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("disabled"));
        let direct = env
            .host
            .handle_web(
                &env.peer.fingerprint(),
                json!({"version":1,"id":"x","method":"workspace.list","params":{}}),
            )
            .await;
        assert!(direct.unwrap_err().contains("disabled"));
    });
}

// (c) the relay restarts on the same port and the desktop reconnects.
#[test]
fn the_desktop_reconnects_after_the_relay_restarts() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let port = stub.port();
    let mut env = env(port);
    env.host.set_web_enabled_with(true, fast()).unwrap();
    let stub = rt.block_on(async {
        let mut conn = stub.accept().await;
        env.peer.handshake(&mut conn, 1).await;
        assert_eq!(
            env.peer.call(&mut conn, 1, "workspace.list").await["ok"],
            true
        );
        drop(conn);
        drop(stub);
        status_is(&env.host, WebState::Offline).await;
        let stub = Stub::bind(port).await;
        let mut conn = within(stub.accept()).await;
        // Sessions did not survive: a new handshake on the same channel works.
        env.peer.handshake(&mut conn, 1).await;
        assert_eq!(
            env.peer.call(&mut conn, 2, "workspace.list").await["ok"],
            true
        );
        status_is(&env.host, WebState::Connected).await;
        stub
    });
    drop(stub);
}

// (d) close 4003 stops reconnecting and reports Revoked.
#[test]
fn close_4003_stops_the_client_and_reports_revoked() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let env = env(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        conn.close(4003).await;
        let status = status_is(&env.host, WebState::Revoked).await;
        assert_eq!(status.error.as_deref(), Some(REVOKED_TEXT));
        assert!(status.enabled);
        // Several backoff periods pass with no new connection.
        let again = tokio::time::timeout(Duration::from_millis(600), stub.accept()).await;
        assert!(again.is_err(), "the client reconnected after 4003");
        assert_eq!(env.host.web_status().state, WebState::Revoked);
    });
}

// (e) only the web path is on: no router, no listener.
#[test]
fn the_router_does_not_start_with_only_web_on() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let env = env(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let _conn = stub.accept().await;
        status_is(&env.host, WebState::Connected).await;
    });
    assert!(!env.host.router_started());
    let status = env.host.status();
    assert!(!status.enabled && !status.listening);
}

// (f) URL validation.
#[test]
fn relay_urls_are_validated() {
    let id = "abc-123";
    assert_eq!(
        relay_url("https://zptah.app", id, false).unwrap(),
        "wss://zptah.app/relay/desktop/abc-123"
    );
    assert_eq!(
        relay_url("https://zptah.app/", id, false).unwrap(),
        "wss://zptah.app/relay/desktop/abc-123"
    );
    assert_eq!(
        relay_url("wss://relay.example.test:8443", id, false).unwrap(),
        "wss://relay.example.test:8443/relay/desktop/abc-123"
    );
    for local in [
        "http://localhost:8787",
        "ws://127.0.0.1:9",
        "http://[::1]:8787",
        "ws://localhost",
    ] {
        assert!(relay_url(local, id, true).is_ok(), "{local}");
        assert!(relay_url(local, id, false).is_err(), "{local} in release");
    }
    assert_eq!(
        relay_url("http://localhost:8787", id, true).unwrap(),
        "ws://localhost:8787/relay/desktop/abc-123"
    );
    for bad in [
        "http://zptah.app",
        "ws://zptah.app",
        "http://localhost.evil.test",
        "http://127.0.0.1.evil.test",
        "https://zptah.app/path",
        "https://zptah.app?x=1",
        "https://zptah.app#frag",
        "https://user:pass@zptah.app",
        "https://",
        "ftp://zptah.app",
        "zptah.app",
        "https://zptah.app:0",
        "https://zptah.app:99999",
        "https://zptah.app:abc",
        "https://a:1:2",
    ] {
        assert!(relay_url(bad, id, true).is_err(), "{bad}");
        assert!(relay_url(bad, id, false).is_err(), "{bad}");
    }
    assert!(relay_url("https://zptah.app", "", true).is_err());
    assert!(relay_url("https://zptah.app", "a/b", true).is_err());
}

// (g) the bearer token is sent, and never shows in the status.
#[test]
fn the_bearer_token_is_sent_and_never_reported() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let env = env(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        assert_eq!(conn.auth, format!("Bearer {TOKEN}"));
        let connected = status_is(&env.host, WebState::Connected).await;
        conn.close(1008).await;
        let stopped = status_is(&env.host, WebState::Offline).await;
        for status in [connected, stopped] {
            assert!(!format!("{status:?}").contains(TOKEN));
        }
        assert!(!format!("{:?}", fast()).contains(TOKEN));
    });
}

// (h) a QUIC lookup of a web fingerprint is refused, and the reverse.
#[test]
fn each_path_serves_only_its_own_kind_of_device() {
    let rt = runtime();
    let env = env(1);
    let phone = Device {
        id: "phone-1".into(),
        name: "Phone".into(),
        fingerprint: "feedfacefeedface".into(),
        kind: DeviceKind::Phone,
        server_device_id: None,
        ..web_row(&env.peer)
    };
    env.host.add_device_for_test(phone.clone());
    env.host.set_flags_for_test(true, true);
    let list = json!({"version":1,"id":"x","method":"workspace.list","params":{}});
    rt.block_on(async {
        let web = env.peer.fingerprint();
        let refused = "Device revoked or not paired";
        assert_eq!(
            env.host.handle(&web, list.clone()).await.unwrap_err(),
            refused
        );
        assert_eq!(
            env.host
                .handle_web(&phone.fingerprint, list.clone())
                .await
                .unwrap_err(),
            refused
        );
        assert!(env.host.handle_web(&web, list.clone()).await.is_ok());
        assert!(env
            .host
            .handle(&phone.fingerprint, list.clone())
            .await
            .is_ok());
    });
    assert!(matches!(
        env.host.web_lookup(&env.peer.fingerprint()),
        Lookup::Confirmed { .. }
    ));
    assert_eq!(env.host.web_lookup(&phone.fingerprint), Lookup::Unknown);
}

#[test]
fn close_codes_follow_the_architecture_table() {
    let mut budget = Budget::new();
    let backoff = |step: &Step| matches!(step, Step::Backoff { .. });
    assert!(backoff(&react(End::Closed(Some(1000)), &mut budget)));
    assert!(backoff(&react(End::Lost, &mut budget)));
    assert!(backoff(&react(End::Closed(Some(4012)), &mut budget)));
    // 1001 and 4009 retry at once the first time only.
    assert_eq!(react(End::Closed(Some(1001)), &mut budget), Step::Immediate);
    assert!(backoff(&react(End::Closed(Some(1001)), &mut budget)));
    assert_eq!(react(End::Closed(Some(4009)), &mut budget), Step::Immediate);
    assert!(backoff(&react(End::Closed(Some(4009)), &mut budget)));
    for code in [4008, 4010] {
        assert!(matches!(
            react(End::Closed(Some(code)), &mut budget),
            Step::Penalty { .. }
        ));
    }
    for code in [1008, 1009] {
        assert!(matches!(
            react(End::Closed(Some(code)), &mut budget),
            Step::Stop {
                state: WebState::Offline,
                ..
            }
        ));
    }
    for end in [
        End::Closed(Some(4001)),
        End::Closed(Some(4003)),
        End::Closed(Some(4011)),
        End::Unauthorized,
    ] {
        assert_eq!(
            react(end, &mut budget),
            Step::Stop {
                state: WebState::Revoked,
                error: REVOKED_TEXT.into()
            }
        );
    }
}

#[test]
fn backoff_doubles_from_one_second_to_a_sixty_second_cap() {
    let timing = Timing::default();
    let seconds: Vec<u64> = (0..10)
        .map(|attempt| backoff_ceiling(&timing, attempt).as_secs())
        .collect();
    assert_eq!(seconds, [1, 2, 4, 8, 16, 32, 60, 60, 60, 60]);
    assert_eq!(backoff_ceiling(&timing, u32::MAX), Duration::from_secs(60));
    assert_eq!(timing.ping, Duration::from_secs(30));
}

#[test]
fn keepalive_ping_noise_and_invalid_frames_are_handled() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let mut env = env(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        // Garbage, text and a short frame are dropped without a reaction.
        conn.ws
            .send(Message::Binary(vec![1, 2, 3].into()))
            .await
            .unwrap();
        conn.ws.send(Message::text("pong")).await.unwrap();
        conn.ws.send(Message::text("anything else")).await.unwrap();
        env.peer.handshake(&mut conn, 1).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        // The reply below also drains the ping texts sent meanwhile.
        assert_eq!(
            env.peer.call(&mut conn, 1, "workspace.list").await["ok"],
            true
        );
        assert!(conn.pings >= 2, "pings: {}", conn.pings);
    });
}

#[test]
fn a_device_left_frame_ends_its_sessions() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let mut env = env(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        env.peer.handshake(&mut conn, 1).await;
        let left = json!({"t":"device","id":env.peer.relay_id(),"event":"left"});
        conn.send(Frame {
            kind: FrameType::Ctrl,
            channel: 0,
            device_id: [0; 16],
            body: serde_json::to_vec(&left).unwrap(),
        })
        .await;
        // The session is gone, so a request on it is answered with a BYE.
        let request = json!({"version":1,"rid":1,"id":"c","method":"workspace.list","params":{}});
        env.peer.write(&mut conn, &request).await;
        let frame = conn.frame().await;
        assert_eq!(frame.kind, FrameType::Bye);
        assert_eq!(
            SessionTable::bye_frame(super::session::SessionKey {
                device_id: env.peer.id,
                channel: 1
            }),
            frame
        );
    });
}

#[test]
fn unpair_stops_the_client_and_no_identity_means_off() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let env = env(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        status_is(&env.host, WebState::Connected).await;
        env.host.web_unpair().unwrap();
        assert!(conn.ended().await);
        let status = env.host.web_status();
        assert_eq!((status.state, status.error), (WebState::Off, None));
        // With the identity wiped, switching on again finds nothing to start.
        env.host.set_web_enabled_with(true, fast()).unwrap();
        let status = status_is(&env.host, WebState::Off).await;
        assert_eq!(status.error, None);
        assert!(status.enabled);
    });
}

// --- The shared serve on the web path (R-06) --------------------------------

const KEY: SessionKey = SessionKey {
    device_id: [7; 16],
    channel: 1,
};

/// `Requests` over a host whose web flag is on, with the app's events.
struct Serving {
    env: Env,
    requests: Requests,
    out: tokio::sync::mpsc::Receiver<Out>,
    events: async_channel::Receiver<Event>,
}

fn serving(capacity: usize) -> Serving {
    let (env, events) = env_with_events(0);
    env.host.set_flags_for_test(false, true);
    let (tx, out) = tokio::sync::mpsc::channel(capacity);
    let requests = Requests::new(env.host.clone(), tx);
    Serving {
        env,
        requests,
        out,
        events,
    }
}

impl Serving {
    fn deliver(&mut self, envelope: Value) {
        let peer = self.env.peer.fingerprint();
        self.requests.deliver(KEY, peer, envelope);
    }
    async fn reply(&mut self) -> Value {
        within(self.out.recv()).await.expect("a reply").value
    }
    async fn request(&self) -> crate::mobile::host::Request {
        match within(self.events.recv()).await.unwrap() {
            Event::Request(request) => request,
            _ => panic!("expected a request"),
        }
    }
    async fn quiet(&self) {
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(self.events.try_recv().is_err(), "the app got a request");
    }
}

fn call(rid: u64, id: &str, method: &str) -> Value {
    json!({"version":1,"rid":rid,"id":id,"method":method,"params":{}})
}

#[test]
fn control_requests_keep_their_order_while_a_read_passes() {
    let rt = runtime();
    let mut s = serving(64);
    rt.block_on(async {
        s.deliver(call(1, "c1", "conversation.send"));
        s.deliver(call(2, "c2", "conversation.send"));
        s.deliver(call(3, "r3", "workspace.list"));
        // The first command is held by the app; the read answers meanwhile.
        let first = s.request().await;
        assert!(first.command_id.ends_with(":c1"));
        let read = s.reply().await;
        assert_eq!(read["rid"], 3);
        assert_eq!(read["ok"], true, "{read}");
        // The second command waits for the first.
        s.quiet().await;
        first.reply(Ok(json!("one")));
        assert_eq!(s.reply().await, json!({"rid":1,"ok":true,"result":"one"}));
        let second = s.request().await;
        assert!(second.command_id.ends_with(":c2"));
        second.reply(Err("nope".into()));
        assert_eq!(s.reply().await, json!({"rid":2,"ok":false,"error":"nope"}));
    });
}

#[test]
fn an_unknown_method_waits_in_the_control_queue() {
    let rt = runtime();
    let mut s = serving(64);
    rt.block_on(async {
        s.deliver(call(1, "c1", "conversation.send"));
        let first = s.request().await;
        s.deliver(call(2, "x", "no.such.method"));
        // Unknown means control: it does not overtake the held command.
        s.quiet().await;
        assert!(s.out.try_recv().is_err());
        first.reply(Ok(Value::Null));
        assert_eq!(s.reply().await["rid"], 1);
        let refused = s.reply().await;
        assert_eq!(refused["rid"], 2);
        assert_eq!(refused["ok"], false);
    });
}

#[test]
fn the_same_command_id_reaches_the_app_under_one_key_and_both_get_the_answer() {
    let rt = runtime();
    let mut s = serving(64);
    rt.block_on(async {
        s.deliver(call(1, "dup", "conversation.send"));
        s.deliver(call(2, "dup", "conversation.send"));
        let first = s.request().await;
        let key = first.command_id.clone();
        // The app deduplicates on this key, as it does for a QUIC retry.
        assert_eq!(key, "web-1:dup");
        first.reply(Ok(json!("done")));
        let second = s.request().await;
        assert_eq!(second.command_id, key);
        second.reply(Ok(json!("done")));
        assert_eq!(s.reply().await, json!({"rid":1,"ok":true,"result":"done"}));
        assert_eq!(s.reply().await, json!({"rid":2,"ok":true,"result":"done"}));
    });
}

#[test]
fn requests_and_watches_carry_the_web_rows_device_context() {
    let rt = runtime();
    let mut s = serving(64);
    rt.block_on(async {
        s.deliver(call(1, "c1", "conversation.send"));
        let request = s.request().await;
        assert_eq!(request.device.kind, DeviceKind::Web);
        assert_eq!(request.device.id, "web-1");
        assert_eq!(request.device.permission, Permission::Control);
        request.reply(Ok(Value::Null));
        s.deliver(
            json!({"version":1,"rid":2,"id":"w","method":"conversation.watch","watch":true,
            "params":{"conversationId":"conv"}}),
        );
        let Event::Watch(start) = within(s.events.recv()).await.unwrap() else {
            panic!("expected a watch");
        };
        assert_eq!(start.device.kind, DeviceKind::Web);
        assert_eq!(start.device.permission, Permission::Control);
        start.accept(Ok(()));
    });
}

#[test]
fn more_than_16_pending_requests_host_wide_are_refused_and_64_per_connection() {
    let rt = runtime();
    let mut s = serving(256);
    // One session may run only 4 reads and hold 16 requests, so the host-wide
    // and per-connection limits are reached over several sessions.
    let session = |n: u64, per: u64| SessionKey {
        device_id: [7; 16],
        channel: (n / per) as u16,
    };
    rt.block_on(async {
        // Reads run concurrently, so they pile up at the app like QUIC streams.
        for n in 0..17u64 {
            let peer = s.env.peer.fingerprint();
            let envelope = call(n, &format!("u{n}"), "accounts.usage");
            s.requests.deliver(session(n, 4), peer, envelope);
        }
        let refused = s.reply().await;
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["error"], "Desktop is busy; try again");
        let mut held = Vec::new();
        while let Ok(Event::Request(request)) =
            tokio::time::timeout(Duration::from_millis(100), s.events.recv())
                .await
                .map_err(|_| ())
                .and_then(|event| event.map_err(|_| ()))
        {
            held.push(request);
        }
        assert_eq!(held.len(), 16);
        for request in held {
            request.reply(Ok(Value::Null));
        }
        for _ in 0..16 {
            assert_eq!(s.reply().await["ok"], true);
        }
        // One connection holds at most 64 requests in flight, queued or not.
        for n in 100..165u64 {
            let peer = s.env.peer.fingerprint();
            let envelope = call(n, &format!("q{n}"), "conversation.send");
            s.requests.deliver(session(n - 100, 13), peer, envelope);
        }
        let busy = s.reply().await;
        assert_eq!(busy["rid"], 164);
        assert_eq!(busy["error"], "Desktop is busy; try again");
    });
}

#[test]
fn one_session_runs_at_most_4_reads_and_holds_at_most_16_requests() {
    let rt = runtime();
    let mut s = serving(256);
    rt.block_on(async {
        // Five reads on one session: the fifth is refused at once with the
        // stable `busy` code, the other four reach the app.
        for n in 0..5u64 {
            s.deliver(call(n, &format!("u{n}"), "accounts.usage"));
        }
        let refused = s.reply().await;
        assert_eq!(
            refused,
            json!({"rid":4,"ok":false,"error":"Desktop is busy; try again","code":"busy"})
        );
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(s.request().await);
        }
        s.quiet().await;
        // The session's slots free up when the reads end.
        for request in held {
            request.reply(Ok(Value::Null));
        }
        for _ in 0..4 {
            assert_eq!(s.reply().await["ok"], true);
        }
        s.deliver(call(5, "again", "accounts.usage"));
        s.request().await.reply(Ok(Value::Null));
        assert_eq!(s.reply().await["rid"], 5);
        // Seventeen queued commands: the 17th is over the session's 16.
        for n in 10..27u64 {
            s.deliver(call(n, &format!("q{n}"), "conversation.send"));
        }
        let busy = s.reply().await;
        assert_eq!(busy["rid"], 26);
        assert_eq!(busy["code"], "busy");
    });
}

#[test]
fn an_unsupported_answer_carries_its_stable_code_on_the_web_channel() {
    let rt = runtime();
    let mut s = serving(64);
    rt.block_on(async {
        s.deliver(call(1, "p", "device.push"));
        let reply = s.reply().await;
        assert_eq!(
            reply,
            json!({"rid":1,"ok":false,"error":"Unsupported mobile operation","code":"unsupported"})
        );
    });
}

#[test]
fn a_watch_whose_updates_are_not_drained_reports_busy_and_cancel_ends_it() {
    let rt = runtime();
    // Room for the accept frame only: the reader never drains the rest.
    let mut s = serving(1);
    rt.block_on(async {
        s.deliver(
            json!({"version":1,"rid":9,"id":"w","method":"conversation.watch","watch":true,
            "params":{"conversationId":"conv"}}),
        );
        let Event::Watch(start) = within(s.events.recv()).await.unwrap() else {
            panic!("expected a watch");
        };
        let id = start.id.clone();
        start.accept(Ok(()));
        let mut busy = false;
        for n in 0..2000 {
            match s.env.host.push(&id, json!({ "n": n })) {
                Err(Push::Busy) => {
                    busy = true;
                    break;
                }
                Ok(()) | Err(Push::Gone) => {}
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(busy, "the sink never reported Busy");
        // Drain, then stop the watch from the browser.
        assert_eq!(s.reply().await, json!({"rid":9,"ok":true,"watching":true}));
        s.deliver(json!({"rid":9,"cancel":true}));
        within(async {
            loop {
                if s.env.host.push(&id, json!({})) == Err(Push::Gone) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
                while s.out.try_recv().is_ok() {}
            }
        })
        .await;
    });
}

// --- Fix wave: liveness and oversized frames -------------------------------

// A relay that goes silent (no pong, nothing) is treated as lost after the
// read deadline, and the desktop dials again.
#[test]
fn a_silent_relay_is_treated_as_lost_after_the_read_deadline() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let env = env(stub.port());
    let timing = Timing {
        read_deadline: Duration::from_millis(300),
        ..fast()
    };
    env.host.set_web_enabled_with(true, timing).unwrap();
    rt.block_on(async {
        // Accepted, then never read from or answered: no pong ever comes.
        let silent = stub.accept().await;
        status_is(&env.host, WebState::Connected).await;
        let started = std::time::Instant::now();
        status_is(&env.host, WebState::Offline).await;
        assert!(started.elapsed() >= Duration::from_millis(200));
        let second = within(stub.accept()).await;
        assert_eq!(second.path, format!("/relay/desktop/{DESKTOP_UUID}"));
        drop(silent);
    });
}

// A watch frame too large to send gets the error frame and then the watch
// ends: the host's sink is dropped, so the app is told Gone.
#[test]
fn a_watch_frame_that_is_too_large_ends_the_watch() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let (mut env, events) = env_with_events(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        env.peer.handshake(&mut conn, 1).await;
        let watch = json!({"version":1,"rid":9,"id":"w","method":"conversation.watch","watch":true,
            "params":{"conversationId":"conv"}});
        env.peer.write(&mut conn, &watch).await;
        let Event::Watch(start) = within(events.recv()).await.unwrap() else {
            panic!("expected a watch");
        };
        let id = start.id.clone();
        start.accept(Ok(()));
        assert_eq!(
            env.peer.read(&mut conn).await,
            json!({"rid":9,"ok":true,"watching":true})
        );
        let huge = json!({ "pad": "x".repeat(9 * 1024 * 1024) });
        env.host.push(&id, huge).unwrap();
        let error = env.peer.read(&mut conn).await;
        assert_eq!(
            error,
            json!({"rid":9,"ok":false,"error":"Response too large"})
        );
        within(async {
            loop {
                if env.host.push(&id, json!({})) == Err(Push::Gone) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
    });
}

// After a too large frame ends a watch, the frames queued behind it for the
// same rid are dropped, not sent after the error.
#[test]
fn nothing_follows_the_error_of_a_watch_ended_for_a_too_large_frame() {
    let rt = runtime();
    let stub = rt.block_on(Stub::bind(0));
    let (mut env, events) = env_with_events(stub.port());
    env.host.set_web_enabled_with(true, fast()).unwrap();
    rt.block_on(async {
        let mut conn = stub.accept().await;
        env.peer.handshake(&mut conn, 1).await;
        let watch = json!({"version":1,"rid":9,"id":"w","method":"conversation.watch","watch":true,
            "params":{"conversationId":"conv"}});
        env.peer.write(&mut conn, &watch).await;
        let Event::Watch(start) = within(events.recv()).await.unwrap() else {
            panic!("expected a watch");
        };
        let id = start.id.clone();
        start.accept(Ok(()));
        env.peer.read(&mut conn).await;
        // The first update goes out at once; the next two are held for one
        // flush and cannot merge (snapshot turns against patches).
        env.host.push(&id, json!({"n": 0})).unwrap();
        assert_eq!(env.peer.read(&mut conn).await["result"], json!({"n": 0}));
        let huge = json!({"turns":[{"id":"a","pad":"x".repeat(9 * 1024 * 1024)}]});
        let small = json!({"turns":[],"patches":[{"id":"a","edits":[]}]});
        env.host.push(&id, huge).unwrap();
        env.host.push(&id, small).unwrap();
        assert_eq!(
            env.peer.read(&mut conn).await,
            json!({"rid":9,"ok":false,"error":"Response too large"})
        );
        let more = tokio::time::timeout(Duration::from_millis(900), conn.frame()).await;
        assert!(more.is_err(), "a frame followed the error");
    });
}

// --- The desktop seam of WS-D3: pairing, grant sync, confirm, deny, revoke ---

mod seam {
    use super::*;
    use crate::mobile::web_link::desktop_api::{self, PollOutcome};
    use crate::mobile::web_link::{PairPoll, WebStatus};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    #[derive(Clone, Debug)]
    struct Req {
        method: String,
        path: String,
        auth: String,
        body: Value,
        port: u16,
    }

    type Handler = Arc<dyn Fn(&Req) -> (u16, String) + Send + Sync>;
    type Seen = Arc<Mutex<Vec<Req>>>;
    type Grants = Arc<Mutex<Vec<Value>>>;

    /// One local port that is both the web server's HTTP API and its relay:
    /// a request to `/relay/...` is a WebSocket from the desktop, anything
    /// else is answered by the handler.
    struct Hub {
        rt: tokio::runtime::Runtime,
        port: u16,
        seen: Seen,
        conns: tokio::sync::Mutex<UnboundedReceiver<Conn>>,
    }

    impl Hub {
        fn start(handler: Handler) -> Hub {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
            let port = listener.local_addr().unwrap().port();
            let (tx, rx) = unbounded_channel();
            let seen: Seen = Arc::default();
            let log = seen.clone();
            rt.spawn(async move {
                loop {
                    let Ok((tcp, _)) = listener.accept().await else {
                        return;
                    };
                    tokio::spawn(serve(tcp, port, tx.clone(), handler.clone(), log.clone()));
                }
            });
            Hub {
                rt,
                port,
                seen,
                conns: tokio::sync::Mutex::new(rx),
            }
        }

        async fn accept(&self) -> Conn {
            within(async { self.conns.lock().await.recv().await.unwrap() }).await
        }

        fn count(&self, method: &str, path: &str) -> usize {
            self.requests(method, path).len()
        }

        fn requests(&self, method: &str, path: &str) -> Vec<Req> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == method && r.path == path)
                .cloned()
                .collect()
        }
    }

    async fn serve(
        mut tcp: TcpStream,
        port: u16,
        conns: tokio::sync::mpsc::UnboundedSender<Conn>,
        handler: Handler,
        log: Seen,
    ) {
        let mut head = [0u8; 16];
        let n = tcp.peek(&mut head).await.unwrap_or(0);
        if head[..n].starts_with(b"GET /relay/") {
            let mut seen = (String::new(), String::new());
            #[allow(clippy::result_large_err)]
            let callback =
                |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                    seen.0 = request.uri().path().to_string();
                    seen.1 = request
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    Ok(response)
                };
            let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, callback).await else {
                return;
            };
            let _ = conns.send(Conn {
                ws,
                path: seen.0,
                auth: seen.1,
                pings: 0,
            });
            return;
        }
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let body_at = loop {
            let Ok(n) = tcp.read(&mut chunk).await else {
                return;
            };
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..body_at]).to_string();
        let mut lines = head.lines();
        let mut request_line = lines.next().unwrap_or_default().split_whitespace();
        let method = request_line.next().unwrap_or_default().to_string();
        let path = request_line.next().unwrap_or_default().to_string();
        let mut auth = String::new();
        let mut length = 0usize;
        for line in lines {
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
            if lower.starts_with("authorization:") {
                auth = line["authorization:".len()..].trim().to_string();
            }
        }
        while buf.len() < body_at + length {
            let Ok(n) = tcp.read(&mut chunk).await else {
                return;
            };
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let body = serde_json::from_slice(&buf[body_at..]).unwrap_or(Value::Null);
        let request = Req {
            method,
            path,
            auth,
            body,
            port,
        };
        let (status, text) = handler(&request);
        log.lock().unwrap().push(request);
        let extra = if status == 429 {
            "Retry-After: 2\r\n"
        } else {
            ""
        };
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len()
        );
        let _ = tcp.write_all(reply.as_bytes()).await;
    }

    fn bearer() -> String {
        format!("Bearer {TOKEN}")
    }

    /// The desktop token routes of a web server that has `grants`.
    fn grant_handler(grants: Grants) -> Handler {
        Arc::new(move |req| {
            if req.path.starts_with("/api/desktop/") && req.auth != bearer() {
                return (401, "{}".into());
            }
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/api/desktop/grants") => {
                    (200, json!({"grants": *grants.lock().unwrap()}).to_string())
                }
                ("POST", p) if p.starts_with("/api/desktop/grants/") => {
                    (200, r#"{"ok":true}"#.into())
                }
                ("DELETE", "/api/desktop/me") => (200, r#"{"ok":true}"#.into()),
                _ => (404, "{}".into()),
            }
        })
    }

    fn grant(peer: &Peer, state: &str, requested: Option<&str>, permission: Option<&str>) -> Value {
        json!({
            "deviceId": peer.relay_id(),
            "deviceName": "Laptop",
            "publicKey": URL_SAFE_NO_PAD.encode(peer.public),
            "fingerprint": peer.fingerprint(),
            "state": state,
            "requestedPermission": requested,
            "permission": permission,
        })
    }

    struct World {
        // The runtime lives inside the hub; the host is dropped first.
        host: MobileHost,
        hub: Hub,
        desktop_public: [u8; 32],
        _dir: tempfile::TempDir,
    }

    /// A host with a store, and optionally a sealed web identity that points at
    /// the hub. The hub is built first so the identity knows its port.
    fn world(handler: Handler, identity: bool) -> World {
        let hub = Hub::start(handler);
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("store.db");
        rusqlite::Connection::open(&database).unwrap();
        crate::accounts::register_database(dir.path(), &database);
        let (host, _events) = MobileHost::open(dir.path().join("mobile-access.json")).unwrap();
        let (private, public) = generate_static().unwrap();
        if identity {
            host.save_web_identity(&WebIdentity {
                static_private: private,
                static_public: public,
                desktop_id: DESKTOP_UUID.into(),
                desktop_token: TOKEN.into(),
                relay_url: format!("http://127.0.0.1:{}", hub.port),
                account_handle: Some("alice".into()),
            })
            .unwrap();
        }
        World {
            host,
            hub,
            desktop_public: public,
            _dir: dir,
        }
    }

    impl World {
        fn peer(&self, id: u8) -> Peer {
            Peer::new(id, self.desktop_public)
        }

        fn status(&self) -> WebStatus {
            self.host.web_status()
        }

        fn row_of(&self, peer: &Peer) -> Device {
            self.status()
                .devices
                .into_iter()
                .find(|d| d.fingerprint == peer.fingerprint())
                .expect("row")
        }

        fn wait_for(&self, what: &str, condition: impl Fn(&World) -> bool) {
            self.hub.rt.block_on(async {
                tokio::time::timeout(WAIT, async {
                    while !condition(self) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            });
        }

        fn connect(&self) -> Conn {
            self.host.set_web_enabled_with(true, fast()).unwrap();
            self.hub.rt.block_on(self.hub.accept())
        }
    }

    fn confirmed_row(peer: &Peer, id: &str, permission: Permission) -> Device {
        Device {
            id: id.into(),
            server_device_id: Some(peer.relay_id()),
            permission,
            ..web_row(peer)
        }
    }

    /// Nothing but pings may come from the desktop within a short time.
    async fn silent(conn: &mut Conn) {
        let waited = tokio::time::timeout(Duration::from_millis(400), async {
            loop {
                match conn.ws.next().await {
                    Some(Ok(Message::Text(_))) => {}
                    other => return other.is_some(),
                }
            }
        })
        .await;
        assert!(waited.is_err(), "the desktop sent something");
    }

    #[test]
    fn a_dev_host_may_be_http_localhost_and_a_release_host_must_be_https() {
        let dev = |url: &str| desktop_api::base_url(url, true);
        let release = |url: &str| desktop_api::base_url(url, false);
        assert_eq!(
            dev("http://localhost:8787").unwrap(),
            "http://localhost:8787"
        );
        assert_eq!(dev("http://127.0.0.1:1/").unwrap(), "http://127.0.0.1:1");
        assert!(release("http://localhost:8787").is_err());
        assert!(dev("http://example.com").is_err());
        assert_eq!(release("https://zptah.app").unwrap(), "https://zptah.app");
        assert_eq!(
            dev("https://zptah.app:8443").unwrap(),
            "https://zptah.app:8443"
        );
        // A stored relay URL reduces to its host, `ws` to `http`.
        assert_eq!(
            release("wss://zptah.app/relay/desktop/abc").unwrap(),
            "https://zptah.app"
        );
        assert_eq!(
            dev("ws://127.0.0.1:9/relay/desktop/x").unwrap(),
            "http://127.0.0.1:9"
        );
        assert!(release("ws://127.0.0.1:9/relay/desktop/x").is_err());
        for bad in [
            "zptah.app",
            "https://u@zptah.app",
            "https://zptah.app/x",
            "",
        ] {
            assert!(dev(bad).is_err(), "{bad}");
        }
        // The host refuses an insecure address before any network call.
        let world = world(grant_handler(Grants::default()), false);
        let error = world
            .host
            .web_pair_start_with("http://localhost:8787", "Mac", false)
            .unwrap_err();
        assert!(error.contains("https"), "{error}");
        assert_eq!(world.hub.seen.lock().unwrap().len(), 0);
    }

    #[test]
    fn the_client_treats_429_as_slow_down_and_never_shows_the_token() {
        let handler: Handler = Arc::new(|_| (429, "{}".into()));
        let world = world(handler, false);
        let base = format!("http://127.0.0.1:{}", world.hub.port);
        let outcome = desktop_api::pair_poll(&base, "dc", "v").unwrap();
        assert!(matches!(
            outcome,
            PollOutcome::SlowDown {
                retry_after: Some(2)
            }
        ));
        // Nothing listens on port 1: the error text carries no secret.
        for error in [
            desktop_api::desktop_grants("http://127.0.0.1:1", TOKEN).unwrap_err(),
            desktop_api::report_grant_state("http://127.0.0.1:1", TOKEN, "d", "revoked", None)
                .unwrap_err(),
            desktop_api::delete_me("http://127.0.0.1:1", TOKEN, Duration::from_secs(2))
                .unwrap_err(),
        ] {
            assert!(!error.contains(TOKEN), "{error}");
        }
        // An answer that is not 2xx is an error that names only the status.
        let error = desktop_api::desktop_grants(&base, TOKEN).unwrap_err();
        assert!(error.contains("429") && !error.contains(TOKEN), "{error}");
    }

    #[test]
    fn pairing_seals_the_identity_switches_the_link_on_and_hides_the_token() {
        let polls = Arc::new(Mutex::new(0usize));
        let counter = polls.clone();
        let handler: Handler = Arc::new(move |req| {
            match req.path.as_str() {
            "/api/pair/start" => (
                200,
                r#"{"userCode":"ABCD-2345","deviceCode":"fake-device-code","confirmationCode":"123456","expiresIn":600,"interval":3}"#.into(),
            ),
            "/api/pair/poll" => {
                let mut polls = counter.lock().unwrap();
                *polls += 1;
                if *polls == 1 {
                    return (200, r#"{"status":"pending"}"#.into());
                }
                (
                    200,
                    json!({"status":"approved","desktopId":DESKTOP_UUID,"desktopToken":TOKEN,
                        "account":{"handle":"alice"},
                        "relayUrl":format!("ws://127.0.0.1:{}/relay/desktop/{DESKTOP_UUID}", req.port)})
                    .to_string(),
                )
            }
            "/api/desktop/grants" => (200, r#"{"grants":[]}"#.into()),
            _ => (404, "{}".into()),
        }
        });
        let world = world(handler, false);
        let host_url = format!("http://127.0.0.1:{}", world.hub.port);
        assert!(!world.status().paired);
        let ticket = world.host.web_pair_start(&host_url, "My Mac").unwrap();
        assert_eq!(ticket.user_code, "ABCD-2345");
        assert_eq!(ticket.confirmation_code, "123456");
        assert_eq!(ticket.interval, 3);
        assert_eq!(ticket.fingerprint.len(), 64);
        assert_eq!(
            ticket.link,
            format!("{host_url}/pair/ABCD-2345#fp={}", ticket.fingerprint)
        );
        // The server received this desktop's key, the challenge of the verifier.
        let start = world.hub.requests("POST", "/api/pair/start").remove(0).body;
        assert_eq!(start["machineName"], "My Mac");
        assert_eq!(start["appVersion"], env!("CARGO_PKG_VERSION"));
        let public = URL_SAFE_NO_PAD
            .decode(start["publicKey"].as_str().unwrap())
            .unwrap();
        assert_eq!(fingerprint(&public), ticket.fingerprint);
        // While pairing is open another start is not blocked, but the status
        // is not paired yet.
        assert_eq!(world.host.web_pair_poll().unwrap(), PairPoll::Pending);
        assert!(!world.status().paired);
        assert_eq!(
            world.host.web_pair_poll().unwrap(),
            PairPoll::Approved {
                account: "alice".into()
            }
        );
        let poll = world.hub.requests("POST", "/api/pair/poll").remove(0).body;
        assert_eq!(poll["deviceCode"], "fake-device-code");
        let verifier = URL_SAFE_NO_PAD
            .decode(poll["verifier"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier)),
            start["challenge"].as_str().unwrap()
        );
        let status = world.status();
        assert!(status.paired && status.enabled);
        assert_eq!(status.account.as_deref(), Some("alice"));
        assert_eq!(status.desktop_id.as_deref(), Some(DESKTOP_UUID));
        assert_eq!(
            status.fingerprint.as_deref(),
            Some(ticket.fingerprint.as_str())
        );
        assert_eq!(status.host.as_deref(), Some(host_url.as_str()));
        assert!(!format!("{status:?}").contains(TOKEN));
        // The identity is sealed with the handle, and the host is stored bare.
        let identity = world.host.load_web_identity().unwrap().unwrap();
        assert_eq!(identity.account_handle.as_deref(), Some("alice"));
        assert_eq!(identity.relay_url, host_url);
        assert_eq!(fingerprint(&identity.static_public), ticket.fingerprint);
        assert!(!format!("{identity:?}").contains(TOKEN));
        // The link started with the token, and a second pairing is refused.
        let conn = world.hub.rt.block_on(world.hub.accept());
        assert_eq!(conn.auth, bearer());
        let error = world.host.web_pair_start(&host_url, "My Mac").unwrap_err();
        assert!(error.contains("Unpair first"), "{error}");
        assert!(world.host.web_pair_poll().is_err());
    }

    #[test]
    fn denied_and_cancelled_pairings_leave_nothing_behind() {
        let handler: Handler = Arc::new(|req| {
            match req.path.as_str() {
            "/api/pair/start" => (
                200,
                r#"{"userCode":"ABCD-2345","deviceCode":"fake-device-code","confirmationCode":"123456","expiresIn":600,"interval":3}"#.into(),
            ),
            _ => (200, r#"{"status":"denied"}"#.into()),
        }
        });
        let world = world(handler, false);
        let host_url = format!("http://127.0.0.1:{}", world.hub.port);
        world.host.web_pair_start(&host_url, "Mac").unwrap();
        assert_eq!(world.host.web_pair_poll().unwrap(), PairPoll::Denied);
        assert!(world.host.web_pair_poll().is_err());
        world.host.web_pair_start(&host_url, "Mac").unwrap();
        world.host.web_pair_cancel();
        assert!(world.host.web_pair_poll().is_err());
        assert!(!world.status().paired);
        assert_eq!(world.host.load_web_identity().unwrap(), None);
    }

    #[test]
    fn grant_sync_lists_pending_skips_forged_fingerprints_and_applies_revokes_and_requests() {
        let grants = Grants::default();
        let world = world(grant_handler(grants.clone()), true);
        let (p1, p2, p3, p4, p5) = (
            world.peer(21),
            world.peer(22),
            world.peer(23),
            world.peer(24),
            world.peer(25),
        );
        let mut forged = grant(&p2, "pending", None, None);
        forged["fingerprint"] = json!("0".repeat(64));
        let mut named = grant(&p1, "pending", None, None);
        named["deviceName"] = json!("Lap\u{202e}top\u{7}   Pro");
        *grants.lock().unwrap() = vec![
            named,
            forged,
            grant(&p3, "revoked", None, None),
            grant(&p4, "confirmed", Some("control"), Some("read")),
            grant(&p5, "confirmed", Some("read"), Some("control")),
        ];
        world
            .host
            .add_device_for_test(confirmed_row(&p3, "row-3", Permission::Control));
        world
            .host
            .add_device_for_test(confirmed_row(&p4, "row-4", Permission::Read));
        world
            .host
            .add_device_for_test(confirmed_row(&p5, "row-5", Permission::Control));
        let mut conn = world.connect();
        world.wait_for("the grants to apply", |w| {
            let s = w.status();
            s.pending.len() == 1 && s.requests.len() == 1 && s.devices.len() == 2
        });
        let status = world.status();
        assert_eq!(status.pending[0].fingerprint, p1.fingerprint());
        assert_eq!(status.pending[0].name, "Laptop Pro");
        assert_eq!(status.pending[0].server_device_id, p1.relay_id());
        assert!(!status.pending[0].online);
        // The revoked grant removed its row; a lowering was applied by itself.
        assert!(status.devices.iter().all(|d| d.id != "row-3"));
        assert_eq!(world.row_of(&p5).permission, Permission::Read);
        world.wait_for("the lowering report", |w| {
            w.hub.count(
                "POST",
                &format!("/api/desktop/grants/{}/state", p5.relay_id()),
            ) == 1
        });
        let report = world
            .hub
            .requests(
                "POST",
                &format!("/api/desktop/grants/{}/state", p5.relay_id()),
            )
            .remove(0);
        assert_eq!(
            report.body,
            json!({"state": "confirmed", "permission": "read"})
        );
        // A raise is a request, not a change.
        let request = &status.requests[0];
        assert_eq!(request.device_id, "row-4");
        assert_eq!(request.server_device_id, p4.relay_id());
        assert_eq!(
            (request.current, request.requested),
            (Permission::Read, Permission::Control)
        );
        assert_eq!(world.row_of(&p4).permission, Permission::Read);
        // A denied request stays hidden until the server asks for something else.
        world.host.web_answer_request("row-4", false).unwrap();
        assert!(world.status().requests.is_empty());
        let sync = |world: &World, conn: &mut Conn| {
            let before = world.hub.count("GET", "/api/desktop/grants");
            let frame = Frame {
                kind: FrameType::Ctrl,
                channel: 0,
                device_id: [0; 16],
                body: serde_json::to_vec(&json!({"t": "grants"})).unwrap(),
            };
            world.hub.rt.block_on(conn.send(frame));
            world.wait_for("a grant sync", |w| {
                w.hub.count("GET", "/api/desktop/grants") > before
            });
            std::thread::sleep(Duration::from_millis(50));
        };
        sync(&world, &mut conn);
        assert!(world.status().requests.is_empty());
        grants.lock().unwrap()[3]["requestedPermission"] = Value::Null;
        sync(&world, &mut conn);
        grants.lock().unwrap()[3]["requestedPermission"] = json!("control");
        sync(&world, &mut conn);
        world.wait_for("the request to return", |w| w.status().requests.len() == 1);
        // Approving applies the permission and reports it.
        world.host.web_answer_request("row-4", true).unwrap();
        assert_eq!(world.row_of(&p4).permission, Permission::Control);
        assert!(world.status().requests.is_empty());
        world.wait_for("the approval report", |w| {
            w.hub
                .requests(
                    "POST",
                    &format!("/api/desktop/grants/{}/state", p4.relay_id()),
                )
                .iter()
                .any(|r| r.body == json!({"state": "confirmed", "permission": "control"}))
        });
        assert!(!format!("{:?}", world.status()).contains(TOKEN));
    }

    #[test]
    fn an_unconfirmed_device_gets_no_method_and_confirm_makes_its_session_ready() {
        let grants = Grants::default();
        let world = world(grant_handler(grants.clone()), true);
        let (pending, unknown) = (world.peer(31), world.peer(32));
        *grants.lock().unwrap() = vec![grant(&pending, "pending", None, None)];
        let mut conn = world.connect();
        world.wait_for("the pending grant", |w| w.status().pending.len() == 1);
        let mut pending = pending;
        let mut unknown = unknown;
        world.hub.rt.block_on(async {
            // An unknown device is refused at the handshake.
            let desktop = *uuid::Uuid::parse_str(DESKTOP_UUID).unwrap().as_bytes();
            let mut init = Initiator::new(
                &unknown.private,
                &unknown.desktop_public,
                &desktop,
                &unknown.id,
                1,
            )
            .unwrap();
            let hs1 = init.write_hs1(br#"{"name":"Stranger"}"#).unwrap();
            conn.send(unknown.frame(FrameType::Hs1, hs1)).await;
            let reply = tokio::time::timeout(Duration::from_millis(400), async {
                loop {
                    match conn.ws.next().await {
                        Some(Ok(Message::Binary(bytes))) => {
                            return Frame::decode(&bytes).unwrap().kind
                        }
                        Some(Ok(_)) => {}
                        _ => return FrameType::Error,
                    }
                }
            })
            .await;
            assert!(matches!(reply, Ok(FrameType::Bye)), "{reply:?}");
            let _ = &mut unknown;
            // A pending device completes the handshake but no method runs.
            let state = pending.handshake(&mut conn, 2).await;
            assert_eq!(state["state"], "pending");
            let request =
                json!({"version":1,"rid":1,"id":"c1","method":"workspace.list","params":{}});
            pending.write(&mut conn, &request).await;
            silent(&mut conn).await;
        });
        world.wait_for("the device to show online", |w| {
            w.status().pending.first().is_some_and(|p| p.online)
        });
        // Confirm: the live session becomes ready and the request now runs.
        let device = world
            .host
            .web_confirm(&pending.fingerprint(), Permission::Read, None, None)
            .unwrap();
        assert_eq!(device.kind, DeviceKind::Web);
        assert_eq!(
            device.server_device_id.as_deref(),
            Some(pending.relay_id().as_str())
        );
        assert_eq!(device.name, "Laptop");
        world.hub.rt.block_on(async {
            let state = pending.read(&mut conn).await;
            assert_eq!(state["state"], "confirmed");
            assert_eq!(state["permission"], "read");
            let reply = pending.call(&mut conn, 2, "workspace.list").await;
            assert_eq!(reply["ok"], true, "{reply}");
        });
        let status = world.status();
        assert!(status.pending.is_empty());
        assert_eq!(status.devices.len(), 1);
        // The row is saved to the settings file, the server hears it.
        let saved = std::fs::read_to_string(world._dir.path().join("mobile-access.json")).unwrap();
        assert!(saved.contains(&pending.fingerprint()));
        world.wait_for("the confirm report", |w| {
            w.hub
                .requests(
                    "POST",
                    &format!("/api/desktop/grants/{}/state", pending.relay_id()),
                )
                .iter()
                .any(|r| r.body == json!({"state": "confirmed", "permission": "read"}))
        });
        // Only a pending fingerprint can be confirmed.
        assert!(world
            .host
            .web_confirm("f".repeat(64).as_str(), Permission::Control, None, None)
            .is_err());
        assert!(world.host.web_deny(&"f".repeat(64)).is_err());
        // A changed permission is sent on the live session at once.
        world
            .host
            .set_device_access(
                &device.id,
                Permission::Control,
                Some(vec!["p".into()]),
                None,
            )
            .unwrap();
        world.hub.rt.block_on(async {
            let state = pending.read(&mut conn).await;
            assert_eq!(state["state"], "confirmed");
            assert_eq!(state["permission"], "control");
        });
        assert_eq!(world.row_of(&pending).projects, Some(vec!["p".to_string()]));
    }

    #[test]
    fn deny_and_revoke_end_the_live_sessions_with_a_bye() {
        let grants = Grants::default();
        let world = world(grant_handler(grants.clone()), true);
        let (denied, revoked) = (world.peer(41), world.peer(42));
        *grants.lock().unwrap() = vec![grant(&denied, "pending", None, None)];
        world
            .host
            .add_device_for_test(confirmed_row(&revoked, "row-r", Permission::Control));
        let mut conn = world.connect();
        world.wait_for("the pending grant", |w| w.status().pending.len() == 1);
        let (mut denied, mut revoked) = (denied, revoked);
        world.hub.rt.block_on(async {
            assert_eq!(denied.handshake(&mut conn, 1).await["state"], "pending");
            assert_eq!(revoked.handshake(&mut conn, 1).await["state"], "confirmed");
        });
        world.host.web_deny(&denied.fingerprint()).unwrap();
        world.hub.rt.block_on(async {
            let frame = conn.frame().await;
            assert_eq!((frame.kind, frame.device_id), (FrameType::Bye, denied.id));
        });
        assert!(world.status().pending.is_empty());
        world.wait_for("the deny report", |w| {
            w.hub
                .requests(
                    "POST",
                    &format!("/api/desktop/grants/{}/state", denied.relay_id()),
                )
                .iter()
                .any(|r| r.body == json!({"state": "revoked"}))
        });
        world.host.web_revoke("row-r").unwrap();
        world.hub.rt.block_on(async {
            let frame = conn.frame().await;
            assert_eq!((frame.kind, frame.device_id), (FrameType::Bye, revoked.id));
        });
        assert!(world.status().devices.is_empty());
        assert!(world.host.web_revoke("row-r").is_err());
        world.wait_for("the revoke report", |w| {
            w.hub
                .requests(
                    "POST",
                    &format!("/api/desktop/grants/{}/state", revoked.relay_id()),
                )
                .iter()
                .any(|r| r.body == json!({"state": "revoked"}))
        });
    }

    #[test]
    fn revoke_works_while_the_server_is_unreachable() {
        // `env` points the identity at a port nothing listens on.
        let env = env(1);
        let started = std::time::Instant::now();
        env.host.web_revoke("web-1").unwrap();
        assert!(env.host.web_status().devices.is_empty());
        assert!(started.elapsed() < Duration::from_secs(5));
        let saved = std::fs::read_to_string(env._dir.path().join("mobile-access.json")).unwrap();
        assert!(!saved.contains(&env.peer.fingerprint()));
    }

    #[test]
    fn unpair_deletes_the_desktop_wipes_the_identity_and_removes_every_web_row() {
        let world = world(grant_handler(Grants::default()), true);
        let (web, phone_peer) = (world.peer(51), world.peer(52));
        world
            .host
            .add_device_for_test(confirmed_row(&web, "row-w", Permission::Control));
        let phone = Device {
            id: "phone-1".into(),
            fingerprint: phone_peer.fingerprint(),
            kind: DeviceKind::Phone,
            server_device_id: None,
            ..web_row(&phone_peer)
        };
        world.host.add_device_for_test(phone);
        assert!(world.status().paired);
        world.host.web_unpair().unwrap();
        let deleted = world.hub.requests("DELETE", "/api/desktop/me");
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].auth, bearer());
        assert_eq!(world.host.load_web_identity().unwrap(), None);
        let status = world.status();
        assert!(!status.paired && status.account.is_none() && status.host.is_none());
        assert!(status.devices.is_empty() && status.pending.is_empty());
        let all: Vec<String> = world
            .host
            .status()
            .devices
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(all, ["phone-1"]);
        let saved = std::fs::read_to_string(world._dir.path().join("mobile-access.json")).unwrap();
        assert!(!saved.contains("row-w") && saved.contains("phone-1"));
        // Safe to repeat, and pairing is possible again.
        world.host.web_unpair().unwrap();
    }

    #[test]
    fn a_phone_keeps_working_with_no_selection_and_narrows_with_one() {
        let env = env(1);
        let phone = Device {
            id: "phone-1".into(),
            name: "Phone".into(),
            fingerprint: "feedfacefeedface".into(),
            kind: DeviceKind::Phone,
            server_device_id: None,
            ..web_row(&env.peer)
        };
        env.host.add_device_for_test(phone.clone());
        env.host.set_flags_for_test(true, true);
        let list = json!({"version":1,"id":"x","method":"workspace.list","params":{}});
        let rt = runtime();
        let updated = env
            .host
            .set_device_access("phone-1", Permission::Control, None, None)
            .unwrap();
        assert_eq!((updated.projects, updated.kind), (None, DeviceKind::Phone));
        assert!(rt
            .block_on(env.host.handle(&phone.fingerprint, list.clone()))
            .is_ok());
        let narrowed = env
            .host
            .set_device_access(
                "phone-1",
                Permission::Control,
                Some(vec!["only".into()]),
                Some(vec![]),
            )
            .unwrap();
        assert_eq!(narrowed.projects, Some(vec!["only".to_string()]));
        assert_eq!(narrowed.monos, Some(vec![]));
        assert!(rt
            .block_on(env.host.handle(&phone.fingerprint, list))
            .is_ok());
        let stored = env
            .host
            .status()
            .devices
            .into_iter()
            .find(|d| d.id == "phone-1")
            .unwrap();
        assert_eq!(stored.projects, Some(vec!["only".to_string()]));
        // The web rows are not the phones' list in `web_status`.
        assert!(env
            .host
            .web_status()
            .devices
            .iter()
            .all(|d| d.id != "phone-1"));
        assert!(env
            .host
            .set_device_access("nobody", Permission::Read, None, None)
            .is_err());
    }
    #[test]
    fn a_relay_url_on_another_host_or_with_another_id_is_refused_and_given_back() {
        let variant = Arc::new(Mutex::new(0usize));
        let pick = variant.clone();
        let handler: Handler = Arc::new(move |req| {
            match req.path.as_str() {
            "/api/pair/start" => (
                200,
                r#"{"userCode":"ABCD-2345","deviceCode":"fake-device-code","confirmationCode":"123456","expiresIn":600,"interval":3}"#.into(),
            ),
            "/api/pair/poll" => {
                let relay = match *pick.lock().unwrap() {
                    0 => format!("ws://127.0.0.1:1/relay/desktop/{DESKTOP_UUID}"),
                    _ => format!("ws://127.0.0.1:{}/relay/desktop/someone-else", req.port),
                };
                (
                    200,
                    json!({"status":"approved","desktopId":DESKTOP_UUID,"desktopToken":TOKEN,
                        "account":{"handle":"alice"},"relayUrl":relay})
                    .to_string(),
                )
            }
            "/api/desktop/me" => (200, r#"{"ok":true}"#.into()),
            _ => (404, "{}".into()),
        }
        });
        let world = world(handler, false);
        let host_url = format!("http://127.0.0.1:{}", world.hub.port);
        for round in 0..2 {
            *variant.lock().unwrap() = round;
            world.host.web_pair_start(&host_url, "Mac").unwrap();
            let error = world.host.web_pair_poll().unwrap_err();
            assert!(error.contains("invalid pairing"), "{error}");
            assert!(!world.status().paired);
            assert_eq!(world.host.load_web_identity().unwrap(), None);
            // The desktop the server made is deleted on the host the user typed.
            let deleted = world.hub.requests("DELETE", "/api/desktop/me");
            assert_eq!(deleted.len(), round + 1);
            assert_eq!(deleted[round].auth, bearer());
        }
    }

    #[test]
    fn a_pending_session_must_use_the_relay_id_its_grant_names() {
        let grants = Grants::default();
        let world = world(grant_handler(grants.clone()), true);
        let mut peer = world.peer(71);
        let mut wrong = grant(&peer, "pending", None, None);
        wrong["deviceId"] = json!(world.peer(72).relay_id());
        *grants.lock().unwrap() = vec![wrong];
        let mut conn = world.connect();
        world.wait_for("the pending grant", |w| w.status().pending.len() == 1);
        world.hub.rt.block_on(async {
            let desktop = *uuid::Uuid::parse_str(DESKTOP_UUID).unwrap().as_bytes();
            let mut init =
                Initiator::new(&peer.private, &peer.desktop_public, &desktop, &peer.id, 1).unwrap();
            let hs1 = init.write_hs1(br#"{"name":"Laptop"}"#).unwrap();
            conn.send(peer.frame(FrameType::Hs1, hs1)).await;
            let frame = conn.frame().await;
            assert_eq!((frame.kind, frame.device_id), (FrameType::Bye, peer.id));
            let _ = &mut peer;
        });
        assert!(!world.status().pending[0].online);
    }

    #[test]
    fn one_key_under_two_device_ids_is_not_listed_and_a_taken_id_is_refused() {
        let grants = Grants::default();
        let world = world(grant_handler(grants.clone()), true);
        let (twin, other, known) = (world.peer(81), world.peer(82), world.peer(83));
        let mut second = grant(&twin, "pending", None, None);
        second["deviceId"] = json!(world.peer(84).relay_id());
        // `other` is pending under the relay id a confirmed row already has.
        let mut clash = grant(&other, "pending", None, None);
        clash["deviceId"] = json!(known.relay_id());
        *grants.lock().unwrap() = vec![grant(&twin, "pending", None, None), second, clash];
        world
            .host
            .add_device_for_test(confirmed_row(&known, "row-k", Permission::Read));
        let _conn = world.connect();
        world.wait_for("the pending list", |w| w.status().pending.len() == 1);
        let pending = world.status().pending;
        assert_eq!(pending[0].fingerprint, other.fingerprint());
        let error = world
            .host
            .web_confirm(&other.fingerprint(), Permission::Read, None, None)
            .unwrap_err();
        assert!(error.contains("already uses"), "{error}");
    }

    #[test]
    fn a_failed_confirm_report_is_repeated_by_the_next_sync() {
        let grants = Grants::default();
        let seen_grants = grants.clone();
        let handler: Handler = Arc::new(move |req| {
            if req.method == "POST" && req.path.starts_with("/api/desktop/grants/") {
                return (500, "{}".into());
            }
            grant_handler(seen_grants.clone())(req)
        });
        let world = world(handler, true);
        let peer = world.peer(91);
        // The server still lists the device `pending` though it is confirmed here.
        *grants.lock().unwrap() = vec![grant(&peer, "pending", None, None)];
        world
            .host
            .add_device_for_test(confirmed_row(&peer, "row-h", Permission::Control));
        let mut conn = world.connect();
        let path = format!("/api/desktop/grants/{}/state", peer.relay_id());
        world.wait_for("the first report", |w| w.hub.count("POST", &path) == 1);
        assert!(world.status().pending.is_empty());
        let frame = Frame {
            kind: FrameType::Ctrl,
            channel: 0,
            device_id: [0; 16],
            body: serde_json::to_vec(&json!({"t": "grants"})).unwrap(),
        };
        world.hub.rt.block_on(conn.send(frame));
        world.wait_for("the repeated report", |w| w.hub.count("POST", &path) == 2);
        for report in world.hub.requests("POST", &path) {
            assert_eq!(
                report.body,
                json!({"state": "confirmed", "permission": "control"})
            );
        }
        // A confirmed grant that shows another permission is healed too.
        grants.lock().unwrap()[0] = grant(&peer, "confirmed", None, Some("read"));
        let frame = Frame {
            kind: FrameType::Ctrl,
            channel: 0,
            device_id: [0; 16],
            body: serde_json::to_vec(&json!({"t": "grants"})).unwrap(),
        };
        world.hub.rt.block_on(conn.send(frame));
        world.wait_for("the permission report", |w| w.hub.count("POST", &path) == 3);
    }
}

#[test]
fn attached_host_is_the_host_given_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let (host, _events) = MobileHost::open(dir.path().join("mobile-access.json")).unwrap();
    super::start(&host);
    let attached = super::attached_host().expect("start registers the host");
    assert_eq!(attached.web_status(), host.web_status());
}
