//! Tests of the relay client against a local stub relay: a tokio-tungstenite
//! server on 127.0.0.1 plays the relay, and the Noise `Initiator` plays the
//! browser. Nothing leaves localhost.
use super::frame::{Frame, FrameType, Plaintext};
use super::noise::{fingerprint, generate_static, Initiator, Transport};
use super::relay::{backoff_ceiling, react, relay_url, Budget, End, Step, Timing, REVOKED_TEXT};
use super::session::{Lookup, SessionTable};
use super::{WebState, WebStatus};
use crate::mobile::host::{Device, MobileHost};
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
        let (tcp, _) = self.listener.accept().await.unwrap();
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
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("store.db");
    rusqlite::Connection::open(&database).unwrap();
    crate::accounts::register_database(dir.path(), &database);
    let host = MobileHost::open(dir.path().join("mobile-access.json"))
        .unwrap()
        .0;
    let (private, public) = generate_static().unwrap();
    host.save_web_identity(&WebIdentity {
        static_private: private,
        static_public: public,
        desktop_id: DESKTOP_UUID.into(),
        desktop_token: TOKEN.into(),
        relay_url: format!("http://127.0.0.1:{port}"),
    })
    .unwrap();
    let peer = Peer::new(7, public);
    host.add_device_for_test(web_row(&peer));
    Env {
        host,
        peer,
        _dir: dir,
    }
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
