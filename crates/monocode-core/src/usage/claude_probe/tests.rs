use super::*;
use crate::accounts::Profile;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
};

const TOKEN: &str = "sk-ant-oat01-fixture";

struct Fixture {
    root: tempfile::TempDir,
    _store: crate::store::Store,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(&root.path().join("monocode.db")).unwrap();
        Self {
            root,
            _store: store,
        }
    }
    fn setup_token(&self) -> Profile {
        let profile = Profile::new("work", self.root.path());
        profile.save_claude_token(TOKEN).unwrap();
        profile
    }
}

/// Serves one canned response on a local port and hands back the request.
fn serve(response: &'static str) -> (TcpListener, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = listener.try_clone().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = server.accept() else {
            return;
        };
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        while let Ok(n) = stream.read(&mut buf) {
            request.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&request);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let _ = stream.write_all(response.as_bytes());
        let _ = tx.send(String::from_utf8_lossy(&request).into_owned());
    });
    (listener, rx)
}
fn point_at(listener: &TcpListener) {
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    TEST_URL.with(|u| *u.borrow_mut() = Some(url));
}

const OK: &str = "HTTP/1.1 200 OK\r\n\
anthropic-ratelimit-unified-5h-utilization: 0.18\r\n\
anthropic-ratelimit-unified-5h-reset: 1794162600\r\n\
anthropic-ratelimit-unified-7d-utilization: 0.64\r\n\
anthropic-ratelimit-unified-7d-reset: 1794351600\r\n\
content-length: 2\r\nconnection: close\r\n\r\n{}";

#[test]
fn parses_unified_headers_into_percent_windows() {
    let headers = [
        ("anthropic-ratelimit-unified-5h-utilization", "0.18"),
        ("anthropic-ratelimit-unified-5h-reset", "1794162600"),
        ("anthropic-ratelimit-unified-7d-utilization", "1.4"),
    ];
    let get = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    };
    let snapshot = parse_headers(get).unwrap();
    assert!(snapshot.manual);
    assert_eq!(snapshot.windows.len(), 2);
    assert!((snapshot.windows[0].used_percent - 18.).abs() < 1e-9);
    assert_eq!(snapshot.windows[0].minutes, 300);
    assert_eq!(snapshot.windows[0].resets_at, Some(1_794_162_600_000));
    // Over-limit readings are clamped, and a missing reset is not invented.
    assert_eq!(snapshot.windows[1].used_percent, 100.);
    assert_eq!(snapshot.windows[1].resets_at, None);
    assert!(parse_headers(|_| None).is_err());
    assert!(parse_headers(|_| Some("NaN".into())).is_err());
}

#[test]
fn background_refresh_never_probes_and_shows_the_last_reading() {
    let fx = Fixture::new();
    let profile = fx.setup_token();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    point_at(&listener);
    let error = crate::usage::fetch_for(Kind::Claude, &profile, None).unwrap_err();
    assert_eq!(error.message, IDLE);
    assert!(!error.unavailable);
    assert!(
        listener.accept().is_err(),
        "background refresh sent a request"
    );

    // After a manual check, background refreshes return its reading offline.
    let (server, requests) = serve(OK);
    point_at(&server);
    let checked = crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap();
    requests.recv().unwrap();
    point_at(&listener);
    let cached = crate::usage::fetch_for(Kind::Claude, &profile, None).unwrap();
    assert!(cached.manual);
    assert_eq!(cached.updated_at, checked.updated_at);
    assert_eq!(cached.windows, checked.windows);
    assert!(
        listener.accept().is_err(),
        "background refresh sent a request"
    );
    let cache = std::fs::read_to_string(
        profile
            .directory(Kind::Claude)
            .unwrap()
            .unwrap()
            .join(CACHE),
    )
    .unwrap();
    assert!(!cache.contains(TOKEN));
}

#[test]
fn changing_the_credential_forgets_the_previous_reading() {
    let fx = Fixture::new();
    let profile = fx.setup_token();
    let cache = profile
        .directory(Kind::Claude)
        .unwrap()
        .unwrap()
        .join(CACHE);
    let (server, requests) = serve(OK);
    point_at(&server);
    crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap();
    requests.recv().unwrap();
    assert!(cache.exists());

    profile
        .save_claude_token("sk-ant-oat01-replacement")
        .unwrap();
    assert!(!cache.exists(), "a new token kept the old reading");
    let error = crate::usage::fetch_for(Kind::Claude, &profile, None).unwrap_err();
    assert_eq!(error.message, IDLE);

    let (server, requests) = serve(OK);
    point_at(&server);
    crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap();
    requests.recv().unwrap();
    assert!(cache.exists());
    profile.remove_claude_token().unwrap();
    assert!(!cache.exists(), "removing the token kept the reading");

    let (server, requests) = serve(OK);
    point_at(&server);
    profile.save_claude_token(TOKEN).unwrap();
    crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap();
    requests.recv().unwrap();
    profile
        .save_claude_credential(
            crate::accounts::ClaudeCredential::ApiKey,
            "sk-ant-api03-fixture",
        )
        .unwrap();
    assert!(!cache.exists(), "switching to an API key kept the reading");
}

#[test]
fn a_reading_written_after_the_token_changed_is_ignored() {
    let fx = Fixture::new();
    let profile = fx.setup_token();
    let cache = profile
        .directory(Kind::Claude)
        .unwrap()
        .unwrap()
        .join(CACHE);
    let (server, requests) = serve(OK);
    point_at(&server);
    crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap();
    requests.recv().unwrap();
    let old = std::fs::read_to_string(&cache).unwrap();
    assert!(!old.contains(TOKEN));

    // A probe that outlived the change writes its reading after `forget`.
    profile
        .save_claude_token("sk-ant-oat01-replacement")
        .unwrap();
    std::fs::write(&cache, &old).unwrap();
    let error = crate::usage::fetch_for(Kind::Claude, &profile, None).unwrap_err();
    assert_eq!(error.message, IDLE);

    // Same after the token is removed or replaced by an API key.
    profile.remove_claude_token().unwrap();
    std::fs::write(&cache, &old).unwrap();
    assert!(crate::usage::fetch_for(Kind::Claude, &profile, None).is_err());
    profile
        .save_claude_credential(
            crate::accounts::ClaudeCredential::ApiKey,
            "sk-ant-api03-fixture",
        )
        .unwrap();
    std::fs::write(&cache, &old).unwrap();
    assert!(crate::usage::fetch_for(Kind::Claude, &profile, None).is_err());
}

#[test]
fn manual_check_sends_one_minimal_haiku_message_with_the_setup_token() {
    let fx = Fixture::new();
    let profile = fx.setup_token();
    let (server, requests) = serve(OK);
    point_at(&server);
    let snapshot = crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap();
    assert_eq!(snapshot.windows.len(), 2);
    let request = requests.recv().unwrap();
    let lower = request.to_ascii_lowercase();
    assert!(request.starts_with("POST /v1/messages "));
    assert!(lower.contains(&format!("authorization: bearer {TOKEN}").to_ascii_lowercase()));
    assert!(lower.contains("anthropic-beta: oauth-2025-04-20"));
    let body: serde_json::Value =
        serde_json::from_str(&request[request.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["max_tokens"], 1);
    assert_eq!(body["messages"][0]["content"], ".");
}

#[test]
fn api_key_accounts_never_probe() {
    let fx = Fixture::new();
    let profile = Profile::new("work", fx.root.path());
    profile
        .save_claude_credential(
            crate::accounts::ClaudeCredential::ApiKey,
            "sk-ant-api03-fixture",
        )
        .unwrap();
    assert_eq!(profile.claude_setup_token_stamped().unwrap(), None);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    point_at(&listener);
    let error = crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap_err();
    assert!(error.message.contains("API key"));
    assert!(listener.accept().is_err());
}

#[test]
fn maps_errors_to_short_messages_without_the_token() {
    for (status, needle) in [
        (401, "setup-token"),
        (403, "not allowed"),
        (429, "rate limiting"),
        (500, "(500)"),
    ] {
        let error = status_error(status);
        assert!(
            error.message.contains(needle),
            "{status}: {}",
            error.message
        );
        assert!(!error.unavailable);
    }
    let fx = Fixture::new();
    let profile = fx.setup_token();
    let (server, requests) =
        serve("HTTP/1.1 401 Unauthorized\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}");
    point_at(&server);
    let error = crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap_err();
    requests.recv().unwrap();
    assert!(error.message.contains("rejected the setup token"));
    assert!(!error.message.contains(TOKEN));
    // A 429 at the limit still carries the windows, so show them.
    let (server, requests) = serve(
        "HTTP/1.1 429 Too Many Requests\r\n\
anthropic-ratelimit-unified-5h-utilization: 1.0\r\n\
content-length: 2\r\nconnection: close\r\n\r\n{}",
    );
    point_at(&server);
    let snapshot = crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap();
    requests.recv().unwrap();
    assert_eq!(snapshot.windows[0].used_percent, 100.);
    // Unreachable host.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    point_at(&closed);
    drop(closed);
    let error = crate::usage::fetch_with(Kind::Claude, &profile, None, true).unwrap_err();
    assert!(error.message.contains("Could not reach"));
}
