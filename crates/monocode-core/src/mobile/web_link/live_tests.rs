//! Live interop smoke (R-05, the "live" half): the real desktop web link, through
//! `MobileHost`, talks over real WebSockets and a local relay stub to the real
//! TypeScript device client of `web/mock-desktop` (a `@noble` Noise initiator, run
//! with node). Ignored by default: it needs `node` and `web/node_modules`, and uses
//! nothing beyond 127.0.0.1.
//!
//! The stub applies the DesktopRoom rules of architecture.md 4.4 that matter here:
//! the desktop path checks the bearer header; the device path takes the device id
//! from the query and does NOT check a cookie (the stub has no Worker in front of
//! it); device frames are forwarded with the header device id overwritten by that
//! id, desktop frames are routed by their header device id; a device may send only
//! HS1, TRANSPORT and BYE; text `ping` is answered `pong`; the desktop gets CTRL
//! `device` joined and left events.
use super::frame::{Frame, FrameType};
use super::noise::{fingerprint, generate_static};
use super::relay::Timing;
use super::WebState;
use crate::mobile::host::{Device, Event, MobileHost};
use crate::mobile::identity_store::WebIdentity;
use crate::mobile::policy::{DeviceKind, Permission};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::{handshake::server::ErrorResponse, http::Response, Message};

const TOKEN: &str = "test-token-not-real";
const DESKTOP_UUID: &str = "11111111-2222-4333-8444-555555555555";
const DEVICE_UUID: &str = "66666666-7777-4888-8999-aaaaaaaaaaaa";
const OTHER_UUID: &str = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);

type Out = UnboundedSender<Message>;

#[derive(Default)]
struct Room {
    next: u64,
    desktop: Option<(u64, Out)>,
    devices: Vec<(u64, [u8; 16], Out)>,
}

type Shared = Arc<Mutex<Room>>;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(text: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

fn send_frame(to: &Out, frame: &Frame) {
    let _ = to.send(Message::Binary(frame.encode().unwrap().into()));
}

fn ctrl_device(room: &Shared, device: [u8; 16], event: &str) {
    let guard = room.lock().unwrap();
    if let Some((_, desktop)) = &guard.desktop {
        let body = json!({"t": "device", "id": uuid::Uuid::from_bytes(device).hyphenated().to_string(), "event": event});
        let frame = Frame {
            kind: FrameType::Ctrl,
            channel: 0,
            device_id: device,
            body: serde_json::to_vec(&body).unwrap(),
        };
        send_frame(desktop, &frame);
    }
}

enum Role {
    Desktop,
    Device([u8; 16]),
}

#[allow(clippy::result_large_err)] // the handshake callback's fixed signature
async fn serve_conn(tcp: TcpStream, room: Shared) {
    let mut seen: Option<Role> = None;
    let callback = |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                    response| {
        let refuse = |status: u16| -> ErrorResponse {
            Response::builder().status(status).body(None).unwrap()
        };
        let path = request.uri().path().to_string();
        let query = request.uri().query().unwrap_or_default().to_string();
        let desktop_path = format!("/relay/desktop/{DESKTOP_UUID}");
        let device_path = format!("/relay/device/{DESKTOP_UUID}");
        if path == desktop_path {
            let auth = request
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok());
            if auth != Some(format!("Bearer {TOKEN}").as_str()) {
                return Err(refuse(401));
            }
            seen = Some(Role::Desktop);
            Ok(response)
        } else if path == device_path {
            let id = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("device="))
                .and_then(|value| uuid::Uuid::parse_str(value).ok());
            match id {
                Some(id) => {
                    seen = Some(Role::Device(*id.as_bytes()));
                    Ok(response)
                }
                None => Err(refuse(400)),
            }
        } else {
            Err(refuse(404))
        }
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, callback).await else {
        return;
    };
    let Some(role) = seen else { return };
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = unbounded_channel::<Message>();
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });
    let conn = {
        let mut guard = room.lock().unwrap();
        guard.next += 1;
        let conn = guard.next;
        match &role {
            Role::Desktop => guard.desktop = Some((conn, tx.clone())),
            Role::Device(id) => guard.devices.push((conn, *id, tx.clone())),
        }
        conn
    };
    if let Role::Device(id) = &role {
        ctrl_device(&room, *id, "joined");
    }
    while let Some(Ok(message)) = stream.next().await {
        match message {
            Message::Text(text) if text.as_str() == "ping" => {
                let _ = tx.send(Message::Text("pong".into()));
            }
            Message::Binary(bytes) => {
                let Ok(mut frame) = Frame::decode(&bytes) else {
                    continue;
                };
                let guard = room.lock().unwrap();
                match &role {
                    Role::Device(id) => {
                        if !matches!(
                            frame.kind,
                            FrameType::Hs1 | FrameType::Transport | FrameType::Bye
                        ) {
                            continue;
                        }
                        frame.device_id = *id;
                        if let Some((_, desktop)) = &guard.desktop {
                            send_frame(desktop, &frame);
                        }
                    }
                    Role::Desktop => {
                        if !matches!(
                            frame.kind,
                            FrameType::Hs2 | FrameType::Transport | FrameType::Bye
                        ) {
                            continue;
                        }
                        for (_, id, device) in &guard.devices {
                            if *id == frame.device_id {
                                send_frame(device, &frame);
                            }
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    match role {
        Role::Desktop => {
            let mut guard = room.lock().unwrap();
            if guard.desktop.as_ref().is_some_and(|(c, _)| *c == conn) {
                guard.desktop = None;
            }
        }
        Role::Device(id) => {
            room.lock().unwrap().devices.retain(|(c, _, _)| *c != conn);
            ctrl_device(&room, id, "left");
        }
    }
}

async fn run_relay(listener: TcpListener, room: Shared) {
    while let Ok((tcp, _)) = listener.accept().await {
        tokio::spawn(serve_conn(tcp, room.clone()));
    }
}

/// The app's side of the host, on a plain thread so the host (and its runtime) is
/// never dropped inside the test runtime. Answers `conversation.read` (echoing the
/// size of a `blob` param) and accepts watches, pushing 3 frames 600 ms apart so the
/// 500 ms pacer sends each one.
struct App {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl App {
    fn spawn(host: MobileHost, events: async_channel::Receiver<Event>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let Ok(event) = events.try_recv() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                match event {
                    Event::Request(request) => {
                        assert_eq!(request.method, "conversation.read");
                        let blob = request.params["blob"].as_str().map(str::len);
                        let result = match blob {
                            Some(len) => json!({"conversationId": "conv-live", "blobLength": len}),
                            None => json!({"conversationId": "conv-live", "turns": []}),
                        };
                        request.reply(Ok(result));
                    }
                    Event::Watch(start) => {
                        let id = start.id.clone();
                        start.accept(Ok(()));
                        let host = host.clone();
                        std::thread::spawn(move || {
                            for seq in 1..=3 {
                                std::thread::sleep(Duration::from_millis(600));
                                let _ = host.push(&id, json!({"seq": seq, "turns": []}));
                            }
                        });
                    }
                    Event::WatchStop(_) => {}
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Script {
    exit: Option<i32>,
    stdout: String,
    stderr: String,
}

async fn run_script(
    port: u16,
    device: &str,
    device_private: &[u8; 32],
    desktop_key: &[u8; 32],
    expect_bye: bool,
) -> Script {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/mobile/web_link/testdata/live-ts-client.ts");
    let mut command = std::process::Command::new("node");
    command
        .current_dir(&root)
        .arg("--experimental-strip-types")
        .arg(&script)
        .arg("--url")
        .arg(format!(
            "ws://127.0.0.1:{port}/relay/device/{DESKTOP_UUID}?device={device}"
        ))
        .args(["--desktop-key", &hex(desktop_key)])
        .args(["--device-private", &hex(device_private)])
        .args(["--desktop-id", DESKTOP_UUID])
        .args(["--device-id", device])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if expect_bye {
        command.arg("--expect-bye");
    }
    let mut child = command.spawn().expect("node must be installed");
    let deadline = std::time::Instant::now() + SCRIPT_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the TypeScript client timed out");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let read = |pipe: Option<&mut dyn std::io::Read>| {
        let mut text = String::new();
        if let Some(pipe) = pipe {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    };
    Script {
        exit: status.code(),
        stdout: read(child.stdout.as_mut().map(|p| p as &mut dyn std::io::Read)),
        stderr: read(child.stderr.as_mut().map(|p| p as &mut dyn std::io::Read)),
    }
}

/// A host with a sealed web identity pointing at the stub, and one confirmed web
/// device holding the TS client's (obviously fake) static key.
struct Live {
    host: MobileHost,
    events: async_channel::Receiver<Event>,
    desktop_public: [u8; 32],
    device_private: [u8; 32],
    _dir: tempfile::TempDir,
}

fn live(port: u16) -> Live {
    let vectors: Value =
        serde_json::from_str(include_str!("testdata/noise_ik_vectors.json")).unwrap();
    let device_private = unhex32(
        vectors["inputs"]["initiator_static_private"]
            .as_str()
            .unwrap(),
    );
    let device_public = unhex32(
        vectors["inputs"]["initiator_static_public"]
            .as_str()
            .unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("store.db");
    rusqlite::Connection::open(&database).unwrap();
    crate::accounts::register_database(dir.path(), &database);
    // QUIC stays off: a fresh config has `enabled: false`.
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
    host.add_device_for_test(Device {
        id: "web-live".into(),
        name: "Live smoke browser".into(),
        fingerprint: fingerprint(&device_public),
        push: None,
        kind: DeviceKind::Web,
        permission: Permission::Control,
        projects: None,
        monos: None,
        server_device_id: Some(DEVICE_UUID.into()),
        created_at: None,
        last_seen: None,
    });
    Live {
        host,
        events,
        desktop_public: public,
        device_private,
        _dir: dir,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// Starts the stub and the desktop link and waits until the desktop is connected.
async fn start() -> (Live, u16, Shared, App) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let room = Shared::default();
    tokio::spawn(run_relay(listener, room.clone()));
    let live = live(port);
    live.host
        .set_web_enabled_with(true, Timing::default())
        .unwrap();
    let app = App::spawn(live.host.clone(), live.events.clone());
    tokio::time::timeout(Duration::from_secs(10), async {
        while live.host.web_status().state != WebState::Connected {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the desktop never connected to the stub relay");
    (live, port, room, app)
}

#[test]
#[ignore = "needs node and web/node_modules"]
fn the_real_ts_client_completes_noise_ik_requests_watch_and_chunks() {
    let rt = runtime();
    let (live, port, _room, app) = rt.block_on(start());
    let script = rt.block_on(run_script(
        port,
        DEVICE_UUID,
        &live.device_private,
        &live.desktop_public,
        false,
    ));
    println!("LIVE_RESULT {}", script.stdout.trim());
    assert_eq!(script.exit, Some(0), "stderr: {}", script.stderr);
    let result: Value = serde_json::from_str(script.stdout.trim()).unwrap();
    assert_eq!(result["state"], "confirmed");
    assert_eq!(result["proto"], 1);
    assert_eq!(result["workspaceListOk"], true);
    assert_eq!(result["readOk"], true);
    assert_eq!(result["watchFrames"], 3);
    assert_eq!(result["watchSeqs"], json!([1, 2, 3]));
    assert_eq!(result["chunkedBytes"], 200 * 1024);
    assert_eq!(live.host.web_status().state, WebState::Connected);
    live.host.shutdown();
    app.finish();
}

#[test]
#[ignore = "needs node and web/node_modules"]
fn the_ts_client_with_an_unknown_key_gets_bye_and_no_state() {
    let rt = runtime();
    let (live, port, _room, app) = rt.block_on(start());
    // A fake key no row holds, and a device id the host has never seen.
    let stranger = [0x5au8; 32];
    let script = rt.block_on(run_script(
        port,
        OTHER_UUID,
        &stranger,
        &live.desktop_public,
        true,
    ));
    println!("LIVE_RESULT {}", script.stdout.trim());
    assert_eq!(script.exit, Some(0), "stderr: {}", script.stderr);
    let result: Value = serde_json::from_str(script.stdout.trim()).unwrap();
    assert_eq!(result["byeReceived"], true);
    assert!(result["state"].is_null());
    live.host.shutdown();
    app.finish();
}
