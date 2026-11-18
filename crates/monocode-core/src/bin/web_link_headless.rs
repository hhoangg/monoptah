//! Headless desktop binary for the web link (E-01 seam of the e2e harness).
//!
//! Commands (parsed by hand, clap is not a dependency):
//!
//! ```text
//! web_link_headless capabilities
//! web_link_headless check-vectors <path-to-vectors.json>
//! web_link_headless pair --server <http(s) url> --data-dir <dir> [--machine-name <name>] [--timeout-secs <n>]
//! web_link_headless serve --data-dir <dir> [--confirm-pending <read-only|control>]
//! ```
//!
//! Exit codes: 0 ok, 1 check-vectors found a failure, 2 usage or IO error, 3 `serve` cannot run yet,
//! 4 pairing was denied, expired or timed out, 5 the server could not be reached.
//!
//! `pair` plays the desktop side of the device authorization of architecture.md 6.4 (`/api/pair/start`, then
//! `/api/pair/poll` until a signed-in user approves the user code) and stores the result in
//! `<data-dir>/web-pairing.json`. It prints JSON lines on stdout and never the token.
//!
//! `serve` is NOT wired yet: the web identity save, grant confirm and grant sync a binary needs are not `pub` in monocode-core. TODO(E-01:WS-D1 R-05, R-15):
//! see `SERVE_NOT_WIRED` below for the exact items. `capabilities` keeps `"serve": false` until then.
use monocode_core::mobile::web_link::frame::{
    split_message, Frame, FrameType, Plaintext, MAX_REQUEST_BYTES,
};
use monocode_core::mobile::web_link::noise::{fingerprint, generate_static};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const USAGE: &str = "usage:
  web_link_headless capabilities
  web_link_headless check-vectors <path-to-vectors.json>
  web_link_headless pair --server <http(s) url> --data-dir <dir> [--machine-name <name>] [--timeout-secs <n>]
  web_link_headless serve --data-dir <dir> [--confirm-pending <read-only|control>]
  web_link_headless --help

  --confirm-pending is for the E2E harness only: it confirms every pending grant on its own. It is never a default.";

const CAPABILITIES_LINE: &str = r#"{"version":1,"serve":false,"pair":true,"reason":"serve needs a pub web identity save, grant confirm and grant sync from WS-D1 (R-05, R-15)","checkVectors":true}"#;
const SERVE_NOT_WIRED: &str = "web_link serve not wired yet: TODO(E-01:WS-D1 R-05, R-15). Public today and enough for the rest of serve: \
store::Store::open (registers the database), MobileHost::open, MobileHost::publish (the catalog), set_web_enabled, \
web_link::start and web_status. Still crate-private: MobileHost::save_web_identity with mobile::identity_store::WebIdentity \
(a binary cannot seal the pairing result), a way to confirm a pending grant, and the R-15 grant sync that fills pending grants.";
const PAIRING_FILE: &str = "web-pairing.json";
const DEFAULT_MACHINE_NAME: &str = "e2e-headless";
const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Consecutive failed HTTP calls (no answer from the server) before `pair` gives up.
const MAX_NETWORK_FAILURES: u32 = 5;

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Capabilities,
    CheckVectors {
        path: PathBuf,
    },
    Pair {
        server: String,
        data_dir: PathBuf,
        machine_name: String,
        timeout: Duration,
    },
    Serve {
        data_dir: PathBuf,
        /// E2E only: the permission a pending grant is confirmed with, without asking.
        confirm_pending: Option<ConfirmPermission>,
    },
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmPermission {
    ReadOnly,
    Control,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let Some(command) = args.first() else {
        return Err("missing command".into());
    };
    let rest = &args[1..];
    match command.as_str() {
        "--help" | "-h" | "help" => Ok(Command::Help),
        "capabilities" => {
            if !rest.is_empty() {
                return Err("capabilities takes no arguments".into());
            }
            Ok(Command::Capabilities)
        }
        "check-vectors" => match rest {
            [path] => Ok(Command::CheckVectors {
                path: PathBuf::from(path),
            }),
            _ => Err("check-vectors takes exactly one path".into()),
        },
        "pair" => parse_pair(rest),
        "serve" => parse_serve(rest),
        other => Err(format!("unknown command: {other}")),
    }
}

/// Reads `--flag value` pairs; every flag in `known` may appear once.
fn flag_values<'a>(
    rest: &'a [String],
    command: &str,
    known: &[&'a str],
) -> Result<Vec<Option<&'a String>>, String> {
    let mut found: Vec<Option<&String>> = vec![None; known.len()];
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        let Some(index) = known.iter().position(|name| *name == flag.as_str()) else {
            return Err(format!("unknown option for {command}: {flag}"));
        };
        let value = iter.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if found[index].replace(value).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    Ok(found)
}

fn parse_pair(rest: &[String]) -> Result<Command, String> {
    let found = flag_values(
        rest,
        "pair",
        &["--server", "--data-dir", "--machine-name", "--timeout-secs"],
    )?;
    let server = found[0].ok_or("pair needs --server")?;
    let data_dir = found[1].ok_or("pair needs --data-dir")?;
    let parsed =
        url::Url::parse(server).map_err(|e| format!("--server is not a valid URL: {e}"))?;
    let host = parsed.host_str().unwrap_or_default();
    match parsed.scheme() {
        "https" if !host.is_empty() => {}
        // Dev builds accept plain http only for localhost (R-05); so does this tool.
        "http" if matches!(host, "localhost" | "127.0.0.1" | "[::1]") => {}
        "http" => return Err("--server may use http:// only for localhost".into()),
        _ => return Err("--server must be an http:// or https:// URL with a host".into()),
    }
    if data_dir.is_empty() {
        return Err("--data-dir must not be empty".into());
    }
    let machine_name = found[2]
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|| DEFAULT_MACHINE_NAME.into());
    if machine_name.is_empty() || machine_name.chars().count() > 64 {
        return Err("--machine-name must have 1 to 64 characters".into());
    }
    let timeout_secs = match found[3] {
        Some(text) => text
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("--timeout-secs must be a positive number")?,
        None => DEFAULT_TIMEOUT_SECS,
    };
    Ok(Command::Pair {
        server: server.trim_end_matches('/').to_string(),
        data_dir: PathBuf::from(data_dir),
        machine_name,
        timeout: Duration::from_secs(timeout_secs),
    })
}

fn parse_serve(rest: &[String]) -> Result<Command, String> {
    let found = flag_values(rest, "serve", &["--data-dir", "--confirm-pending"])?;
    let data_dir = found[0].ok_or("serve needs --data-dir")?;
    if data_dir.is_empty() {
        return Err("--data-dir must not be empty".into());
    }
    let confirm_pending = match found[1].map(String::as_str) {
        None => None,
        Some("read-only") => Some(ConfirmPermission::ReadOnly),
        Some("control") => Some(ConfirmPermission::Control),
        Some(_) => return Err("--confirm-pending must be read-only or control".into()),
    };
    Ok(Command::Serve {
        data_dir: PathBuf::from(data_dir),
        confirm_pending,
    })
}

// ---------------------------------------------------------------- pair

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// One JSON call. `Ok((status, body))` for any HTTP answer, `Err` when there was none.
fn post_json(url: &str, body: &Value) -> Result<(u16, Value, Option<u64>), String> {
    let request = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build()
        .post(url)
        .set("Content-Type", "application/json");
    let response = match request.send_string(&body.to_string()) {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(e)) => return Err(e.to_string()),
    };
    let status = response.status();
    let retry_after = response
        .header("Retry-After")
        .and_then(|value| value.trim().parse::<u64>().ok());
    let text = response.into_string().map_err(|e| e.to_string())?;
    Ok((
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        retry_after,
    ))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

fn emit(out: &mut dyn Write, line: Value) {
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The desktop half of the device authorization. `poll_every` overrides the server's interval (tests only).
fn run_pair(
    server: &str,
    data_dir: &Path,
    machine_name: &str,
    timeout: Duration,
    poll_every: Option<Duration>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    use rand::RngCore;
    use sha2::{Digest, Sha256};
    if let Err(e) = std::fs::create_dir_all(data_dir) {
        let _ = writeln!(err, "error: cannot create {}: {e}", data_dir.display());
        return 2;
    }
    let (private, public) = match generate_static() {
        Ok(pair) => pair,
        Err(e) => {
            let _ = writeln!(err, "error: {e}");
            return 2;
        }
    };
    let mut verifier = [0u8; 32];
    rand::rng().fill_bytes(&mut verifier);
    let challenge = Sha256::digest(verifier);
    let started = Instant::now();
    let start_body = json!({
        "publicKey": b64(&public),
        "machineName": machine_name,
        "os": os_name(),
        "appVersion": concat!(env!("CARGO_PKG_VERSION"), "-headless"),
        "challenge": b64(&challenge),
    });
    let (status, started_json, _) =
        match post_json(&format!("{server}/api/pair/start"), &start_body) {
            Ok(answer) => answer,
            Err(e) => {
                let _ = writeln!(err, "error: cannot reach the server: {e}");
                return 5;
            }
        };
    let text = |key: &str| started_json.get(key).and_then(Value::as_str);
    let (Some(user_code), Some(device_code), Some(confirmation)) = (
        text("userCode"),
        text("deviceCode"),
        text("confirmationCode"),
    ) else {
        let _ = writeln!(err, "error: pair/start answered {status} without a pairing");
        return 2;
    };
    let interval = poll_every.unwrap_or_else(|| {
        Duration::from_secs(
            started_json
                .get("interval")
                .and_then(Value::as_u64)
                .unwrap_or(3)
                .max(1),
        )
    });
    // The device code and the verifier stay in this function: they are never printed.
    emit(
        out,
        json!({
            "event": "pairing",
            "userCode": user_code,
            "confirmationCode": confirmation,
            "fingerprint": fingerprint(&public),
            "expiresIn": started_json.get("expiresIn"),
        }),
    );
    let poll_body = json!({"deviceCode": device_code, "verifier": b64(&verifier)});
    let mut failures = 0;
    loop {
        if started.elapsed() >= timeout {
            let _ = writeln!(err, "error: pairing timed out");
            return 4;
        }
        let mut wait = interval;
        match post_json(&format!("{server}/api/pair/poll"), &poll_body) {
            Err(e) => {
                failures += 1;
                if failures >= MAX_NETWORK_FAILURES {
                    let _ = writeln!(err, "error: the server stopped answering: {e}");
                    return 5;
                }
            }
            Ok((code, body, retry_after)) => {
                failures = 0;
                match body.get("status").and_then(Value::as_str) {
                    Some("approved") => {
                        return store_pairing(&body, &private, &public, data_dir, out, err);
                    }
                    Some("denied" | "expired") => {
                        let _ = writeln!(err, "error: pairing was {}", body["status"]);
                        return 4;
                    }
                    Some("pending" | "slow_down") => {}
                    _ if code == 429 => {
                        wait = wait.max(Duration::from_secs(retry_after.unwrap_or(0)));
                    }
                    _ => {
                        let _ = writeln!(err, "error: pair/poll answered {code}");
                        return 2;
                    }
                }
            }
        }
        std::thread::sleep(wait);
    }
}

fn store_pairing(
    body: &Value,
    private: &[u8; 32],
    public: &[u8; 32],
    data_dir: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let text = |key: &str| body.get(key).and_then(Value::as_str);
    let handle = body["account"]["handle"].as_str();
    let (Some(desktop_id), Some(token), Some(relay_url), Some(handle)) = (
        text("desktopId"),
        text("desktopToken"),
        text("relayUrl"),
        handle,
    ) else {
        let _ = writeln!(err, "error: the approval is missing fields");
        return 2;
    };
    // E2E stand-in for the sealed web identity (identity_store::WebIdentity, not reachable from a binary):
    // the same fields, in a 0600 file inside the throwaway data dir. It holds the token: never commit it.
    let record = json!({
        "format": "web-link-headless-pairing-v1",
        "desktopId": desktop_id,
        "desktopToken": token,
        "relayUrl": relay_url,
        "account": handle,
        "staticPrivate": b64(private),
        "staticPublic": b64(public),
    });
    if let Err(e) = write_private(&data_dir.join(PAIRING_FILE), record.to_string().as_bytes()) {
        let _ = writeln!(err, "error: cannot save the pairing: {e}");
        return 2;
    }
    emit(
        out,
        json!({
            "event": "paired",
            "desktopId": desktop_id,
            "account": handle,
            "fingerprint": fingerprint(public),
        }),
    );
    0
}

// ---------------------------------------------------------------- check-vectors

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(format!("bad hex string of length {}", text.len()));
    }
    (0..text.len() / 2)
        .map(|i| {
            u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|e| format!("bad hex: {e}"))
        })
        .collect()
}

/// `{"hex"}` or `{"prefix"?, "fill", "length"}`: the prefix, then the fill byte repeated up to `length` bytes.
fn bytes(spec: &Value) -> Result<Vec<u8>, String> {
    if let Some(hex) = spec.get("hex").and_then(Value::as_str) {
        return unhex(hex);
    }
    let mut out = match spec.get("prefix").and_then(Value::as_str) {
        Some(prefix) => unhex(prefix)?,
        None => Vec::new(),
    };
    let fill = unhex(
        spec.get("fill")
            .and_then(Value::as_str)
            .ok_or("byte string needs hex or fill")?,
    )?;
    let [fill] = fill[..] else {
        return Err("fill must be one byte".into());
    };
    let length = spec
        .get("length")
        .and_then(Value::as_u64)
        .ok_or("fill needs a length")? as usize;
    if length < out.len() {
        return Err("fill length is shorter than the prefix".into());
    }
    out.resize(length, fill);
    Ok(out)
}

fn uuid(text: &str) -> Result<[u8; 16], String> {
    unhex(&text.replace('-', ""))?
        .try_into()
        .map_err(|_| format!("bad uuid {text}"))
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value.get(key).ok_or_else(|| format!("missing field {key}"))
}

fn str_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("field {key} is not a string"))
}

fn u64_field(value: &Value, key: &str) -> Result<u64, String> {
    field(value, key)?
        .as_u64()
        .ok_or_else(|| format!("field {key} is not a number"))
}

fn list<'a>(root: &'a Value, group: &str, key: &str) -> Result<&'a Vec<Value>, String> {
    field(field(root, group)?, key)?
        .as_array()
        .ok_or_else(|| format!("{group}.{key} is not an array"))
}

fn check_valid_frame(item: &Value) -> Result<(), String> {
    let wire = bytes(field(item, "frame")?)?;
    let decoded = field(item, "decoded")?;
    let type_code =
        u8::try_from(u64_field(decoded, "typeCode")?).map_err(|_| "typeCode out of range")?;
    let kind = FrameType::from_code(type_code).ok_or("unknown typeCode")?;
    if kind.name() != str_field(decoded, "type")? {
        return Err("type name does not match typeCode".into());
    }
    let frame = Frame {
        kind,
        channel: u16::try_from(u64_field(decoded, "channel")?)
            .map_err(|_| "channel out of range")?,
        device_id: uuid(str_field(decoded, "deviceId")?)?,
        body: bytes(field(decoded, "body")?)?,
    };
    let encoded = frame
        .encode()
        .map_err(|e| format!("encode failed: {}", e.as_str()))?;
    if encoded != wire {
        return Err("encode(decoded) differs from the frame bytes".into());
    }
    let back = Frame::decode(&wire).map_err(|e| format!("decode failed: {}", e.as_str()))?;
    if back != frame {
        return Err("decode(frame) differs from decoded".into());
    }
    Ok(())
}

fn check_invalid_frame(item: &Value) -> Result<(), String> {
    let want = str_field(item, "error")?;
    match Frame::decode(&bytes(field(item, "frame")?)?) {
        Ok(_) => Err(format!("decoded fine, expected {want}")),
        Err(e) if e.as_str() == want => Ok(()),
        Err(e) => Err(format!("failed with {}, expected {want}", e.as_str())),
    }
}

fn expected_plaintext(decoded: &Value) -> Result<Plaintext, String> {
    Ok(match str_field(decoded, "kind")? {
        "msg" => Plaintext::Msg(bytes(field(decoded, "payload")?)?),
        "chunk" => Plaintext::Chunk {
            msg_id: u32::try_from(u64_field(decoded, "msgId")?)
                .map_err(|_| "msgId out of range")?,
            index: u16::try_from(u64_field(decoded, "index")?).map_err(|_| "index out of range")?,
            last: field(decoded, "last")?
                .as_bool()
                .ok_or("last is not a bool")?,
            data: bytes(field(decoded, "data")?)?,
        },
        "rekey" => Plaintext::Rekey,
        other => return Err(format!("unknown plaintext kind {other}")),
    })
}

fn check_valid_plaintext(item: &Value) -> Result<(), String> {
    let wire = bytes(field(item, "plaintext")?)?;
    let expected = expected_plaintext(field(item, "decoded")?)?;
    let encoded = expected
        .encode()
        .map_err(|e| format!("encode failed: {}", e.as_str()))?;
    if encoded != wire {
        return Err("encode(decoded) differs from the plaintext bytes".into());
    }
    let back = Plaintext::decode(&wire).map_err(|e| format!("decode failed: {}", e.as_str()))?;
    if back != expected {
        return Err("decode(plaintext) differs from decoded".into());
    }
    Ok(())
}

fn check_invalid_plaintext(item: &Value) -> Result<(), String> {
    let want = str_field(item, "error")?;
    match Plaintext::decode(&bytes(field(item, "plaintext")?)?) {
        Ok(_) => Err(format!("decoded fine, expected {want}")),
        Err(e) if e.as_str() == want => Ok(()),
        Err(e) => Err(format!("failed with {}, expected {want}", e.as_str())),
    }
}

fn check_split(item: &Value) -> Result<(), String> {
    let fill = unhex(str_field(item, "messageFill")?)?;
    let [fill] = fill[..] else {
        return Err("messageFill must be one byte".into());
    };
    let message = vec![fill; u64_field(item, "messageLength")? as usize];
    let msg_id = u32::try_from(u64_field(item, "msgId")?).map_err(|_| "msgId out of range")?;
    let want: Vec<u64> = field(item, "partLengths")?
        .as_array()
        .ok_or("partLengths is not an array")?
        .iter()
        .map(|v| v.as_u64().ok_or("partLength is not a number"))
        .collect::<Result<_, _>>()?;
    let parts = split_message(&message, msg_id, MAX_REQUEST_BYTES)
        .map_err(|_| "split_message refused the message")?;
    let got: Vec<u64> = parts.iter().map(|p| p.len() as u64).collect();
    if got != want {
        return Err(format!("part lengths {got:?}, expected {want:?}"));
    }
    for (i, part) in parts.iter().enumerate() {
        let decoded = Plaintext::decode(part)
            .map_err(|e| format!("part {i} does not decode: {}", e.as_str()))?;
        let last = i + 1 == parts.len();
        match (parts.len(), decoded) {
            (1, Plaintext::Msg(_)) => {}
            (
                n,
                Plaintext::Chunk {
                    msg_id: id,
                    index,
                    last: l,
                    ..
                },
            ) if n > 1 => {
                if id != msg_id || usize::from(index) != i || l != last {
                    return Err(format!("part {i} has the wrong chunk header"));
                }
            }
            _ => return Err(format!("part {i} has the wrong kind")),
        }
    }
    Ok(())
}

/// Runs one group of vector checks and records the failures. Returns how many items ran.
fn run_group(
    label: &str,
    items: Result<&Vec<Value>, String>,
    check: fn(&Value) -> Result<(), String>,
    failures: &mut Vec<Value>,
) -> usize {
    let items = match items {
        Ok(items) => items,
        Err(e) => {
            failures.push(json!({"group": label, "name": null, "error": e}));
            return 0;
        }
    };
    if items.is_empty() {
        failures.push(json!({"group": label, "name": null, "error": "the group is empty"}));
    }
    for item in items {
        if let Err(e) = check(item) {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
            failures.push(json!({"group": label, "name": name, "error": e}));
        }
    }
    items.len()
}

/// Validates the frame and plaintext vectors of `vectors.json`. Returns the summary and whether all passed.
fn check_vectors(root: &Value) -> (Value, bool) {
    let mut failures = Vec::new();
    let frames_valid = run_group(
        "frames.valid",
        list(root, "frames", "valid"),
        check_valid_frame,
        &mut failures,
    );
    let frames_invalid = run_group(
        "frames.invalid",
        list(root, "frames", "invalid"),
        check_invalid_frame,
        &mut failures,
    );
    let plaintext_valid = run_group(
        "plaintext.valid",
        list(root, "plaintext", "valid"),
        check_valid_plaintext,
        &mut failures,
    );
    let plaintext_invalid = run_group(
        "plaintext.invalid",
        list(root, "plaintext", "invalid"),
        check_invalid_plaintext,
        &mut failures,
    );
    let plaintext_split = run_group(
        "plaintext.split",
        list(root, "plaintext", "split"),
        check_split,
        &mut failures,
    );
    let ok = failures.is_empty();
    let summary = json!({
        "ok": ok,
        "checked": {
            "framesValid": frames_valid,
            "framesInvalid": frames_invalid,
            "plaintextValid": plaintext_valid,
            "plaintextInvalid": plaintext_invalid,
            "plaintextSplit": plaintext_split,
        },
        "failures": failures,
    });
    (summary, ok)
}

// ---------------------------------------------------------------- entry

/// Runs a command; returns the exit code. Output goes to `out` and `err` so tests can capture it.
fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let command = match parse_args(args) {
        Ok(Command::Help) => {
            let _ = writeln!(out, "{USAGE}");
            return 2;
        }
        Ok(command) => command,
        Err(message) => {
            let _ = writeln!(err, "error: {message}\n{USAGE}");
            return 2;
        }
    };
    match command {
        Command::Help => unreachable!("handled above"),
        Command::Capabilities => {
            let _ = writeln!(out, "{CAPABILITIES_LINE}");
            0
        }
        Command::CheckVectors { path } => run_check_vectors(&path, out, err),
        Command::Pair {
            server,
            data_dir,
            machine_name,
            timeout,
        } => run_pair(&server, &data_dir, &machine_name, timeout, None, out, err),
        Command::Serve { data_dir, .. } => {
            // The pairing must exist first; its token is never printed.
            if !data_dir.join(PAIRING_FILE).is_file() {
                let _ = writeln!(
                    err,
                    "error: {} has no {PAIRING_FILE}: run `pair` first",
                    data_dir.display()
                );
                return 2;
            }
            let _ = writeln!(err, "{SERVE_NOT_WIRED}");
            3
        }
    }
}

fn run_check_vectors(path: &Path, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            let _ = writeln!(err, "error: cannot read {}: {e}", path.display());
            return 2;
        }
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(root) => root,
        Err(e) => {
            let _ = writeln!(err, "error: {} is not valid JSON: {e}", path.display());
            return 2;
        }
    };
    let (summary, ok) = check_vectors(&root);
    let _ = writeln!(out, "{summary}");
    if ok {
        0
    } else {
        1
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let code = run(&args, &mut stdout.lock(), &mut stderr.lock());
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn vectors_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/protocol/vectors.json")
    }

    fn run_capture(list: &[&str]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args(list), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn parses_each_command() {
        assert_eq!(
            parse_args(&args(&["capabilities"])),
            Ok(Command::Capabilities)
        );
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(
            parse_args(&args(&["check-vectors", "v.json"])),
            Ok(Command::CheckVectors {
                path: "v.json".into()
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "pair",
                "--server",
                "http://localhost:8787/",
                "--data-dir",
                "d"
            ])),
            Ok(Command::Pair {
                server: "http://localhost:8787".into(),
                data_dir: "d".into(),
                machine_name: DEFAULT_MACHINE_NAME.into(),
                timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            })
        );
        // Option order does not matter; https works for any host.
        assert_eq!(
            parse_args(&args(&[
                "pair",
                "--timeout-secs",
                "30",
                "--machine-name",
                " My PC ",
                "--data-dir",
                "d",
                "--server",
                "https://zptah.example"
            ])),
            Ok(Command::Pair {
                server: "https://zptah.example".into(),
                data_dir: "d".into(),
                machine_name: "My PC".into(),
                timeout: Duration::from_secs(30),
            })
        );
        assert_eq!(
            parse_args(&args(&["serve", "--data-dir", "d"])),
            Ok(Command::Serve {
                data_dir: "d".into(),
                confirm_pending: None
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "serve",
                "--confirm-pending",
                "read-only",
                "--data-dir",
                "d"
            ])),
            Ok(Command::Serve {
                data_dir: "d".into(),
                confirm_pending: Some(ConfirmPermission::ReadOnly)
            })
        );
        assert!(matches!(
            parse_args(&args(&[
                "serve",
                "--data-dir",
                "d",
                "--confirm-pending",
                "control"
            ])),
            Ok(Command::Serve {
                confirm_pending: Some(ConfirmPermission::Control),
                ..
            })
        ));
    }

    #[test]
    fn rejects_bad_arguments() {
        for bad in [
            &[][..],
            &["nope"],
            &["capabilities", "extra"],
            &["check-vectors"],
            &["check-vectors", "a", "b"],
            &["pair"],
            &["pair", "--server", "http://localhost:1"],
            &["pair", "--data-dir", "d"],
            // plain http only for localhost
            &["pair", "--server", "http://example.com", "--data-dir", "d"],
            &["pair", "--server", "ws://localhost:1", "--data-dir", "d"],
            &["pair", "--server", "not a url", "--data-dir", "d"],
            &["pair", "--server", "http://localhost:1", "--data-dir", ""],
            &[
                "pair",
                "--server",
                "http://localhost:1",
                "--data-dir",
                "d",
                "--timeout-secs",
                "0",
            ],
            &[
                "pair",
                "--server",
                "http://localhost:1",
                "--data-dir",
                "d",
                "--machine-name",
                " ",
            ],
            &[
                "pair",
                "--server",
                "http://localhost:1",
                "--server",
                "http://localhost:2",
                "--data-dir",
                "d",
            ],
            &["serve"],
            &["serve", "--data-dir"],
            &["serve", "--data-dir", "d", "--confirm-pending", "admin"],
            &["serve", "--data-dir", "d", "--confirm-pending"],
            &["serve", "--data-dir", "d", "--token", "secret"],
            // the old relay/token-file flags are gone: the relay URL comes from the pairing
            &["serve", "--data-dir", "d", "--relay", "ws://x"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn capabilities_prints_one_json_line() {
        let (code, out, _) = run_capture(&["capabilities"]);
        assert_eq!(code, 0);
        assert_eq!(out.lines().count(), 1);
        let value: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(
            value,
            json!({"version": 1, "serve": false, "pair": true, "reason": "serve needs a pub web identity save, grant confirm and grant sync from WS-D1 (R-05, R-15)", "checkVectors": true})
        );
    }

    #[test]
    fn help_and_unknown_commands_exit_2() {
        assert_eq!(run_capture(&["--help"]).0, 2);
        let (code, _, err) = run_capture(&["bogus"]);
        assert_eq!(code, 2);
        assert!(err.contains("usage:"));
    }

    #[test]
    fn check_vectors_passes_on_the_real_file() {
        let path = vectors_path();
        let (code, out, err) = run_capture(&["check-vectors", path.to_str().unwrap()]);
        assert_eq!(code, 0, "stdout: {out} stderr: {err}");
        let summary: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(summary["ok"], json!(true));
        assert!(summary["checked"]["framesValid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["framesInvalid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["plaintextValid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["plaintextSplit"].as_u64().unwrap() > 0);
    }

    #[test]
    fn check_vectors_fails_on_a_corrupted_copy() {
        let mut root: Value =
            serde_json::from_str(&std::fs::read_to_string(vectors_path()).unwrap()).unwrap();
        // Change the channel of the first valid frame: encode(decoded) no longer equals the bytes.
        root["frames"]["valid"][0]["decoded"]["channel"] = json!(4242);
        // And break the expected error of the first invalid frame.
        root["frames"]["invalid"][0]["error"] = json!("too_long");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vectors.json");
        std::fs::write(&path, serde_json::to_string(&root).unwrap()).unwrap();
        let (code, out, _) = run_capture(&["check-vectors", path.to_str().unwrap()]);
        assert_eq!(code, 1, "{out}");
        let summary: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(summary["ok"], json!(false));
        assert_eq!(summary["failures"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn check_vectors_exit_2_on_io_error_and_bad_json() {
        assert_eq!(
            run_capture(&["check-vectors", "/nonexistent/vectors.json"]).0,
            2
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(run_capture(&["check-vectors", path.to_str().unwrap()]).0, 2);
    }

    #[test]
    fn serve_needs_a_pairing_and_then_reports_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap();
        let (code, _, err) = run_capture(&["serve", "--data-dir", data]);
        assert_eq!(code, 2);
        assert!(err.contains("run `pair` first"));
        std::fs::write(
            dir.path().join(PAIRING_FILE),
            r#"{"desktopToken":"obviously-fake-token-value"}"#,
        )
        .unwrap();
        let (code, out, err) = run_capture(&["serve", "--data-dir", data]);
        assert_eq!(code, 3);
        assert!(err.contains("TODO(E-01:WS-D1 R-05, R-15)"));
        assert!(!out.contains("obviously-fake") && !err.contains("obviously-fake"));
    }

    // ------------------------------------------------------------ pair against a local HTTP stub

    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(String, Value)>>>;

    /// A one-thread HTTP stub of `/api/pair/start` and `/api/pair/poll`. Answers each poll from `polls`
    /// (a status line and a body); records every request body.
    fn stub(polls: Vec<(u16, &'static str)>) -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen: Seen = Arc::default();
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut polls = polls.into_iter();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let path = request_line.split_whitespace().nth(1).unwrap().to_string();
                log.lock().unwrap().push((
                    path.clone(),
                    serde_json::from_slice(&body).unwrap_or(Value::Null),
                ));
                let (status, text) = if path == "/api/pair/start" {
                    (
                        200,
                        r#"{"userCode":"ABCD-2345","deviceCode":"obviously-fake-device-code","confirmationCode":"123456","expiresIn":600,"interval":3}"#,
                    )
                } else {
                    polls.next().unwrap_or((500, "{}"))
                };
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (base, seen)
    }

    fn pair_capture(base: &str, dir: &Path, timeout: Duration) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_pair(
            base,
            dir,
            "stub-machine",
            timeout,
            Some(Duration::from_millis(10)),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn pair_prints_the_user_code_polls_and_stores_the_identity() {
        let (base, seen) = stub(vec![
            (200, r#"{"status":"pending"}"#),
            (429, r#"{"error":"slow","code":"rate_limited"}"#),
            (
                200,
                r#"{"status":"approved","desktopId":"11111111-2222-4333-8444-555555555555","desktopToken":"obviously-fake-desktop-token","account":{"handle":"alice"},"relayUrl":"ws://127.0.0.1:1/relay/desktop/x"}"#,
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let (code, out, err) = pair_capture(&base, dir.path(), Duration::from_secs(20));
        assert_eq!(code, 0, "stderr: {err}");
        let lines: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["event"], "pairing");
        assert_eq!(lines[0]["userCode"], "ABCD-2345");
        assert_eq!(lines[0]["confirmationCode"], "123456");
        assert_eq!(lines[1]["event"], "paired");
        assert_eq!(lines[1]["account"], "alice");
        // The token and the device code never reach stdout or stderr.
        let output = format!("{out}{err}");
        assert!(!output.contains("obviously-fake"));
        // The stored identity is complete.
        let stored: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(PAIRING_FILE)).unwrap())
                .unwrap();
        assert_eq!(stored["desktopToken"], "obviously-fake-desktop-token");
        assert_eq!(stored["desktopId"], "11111111-2222-4333-8444-555555555555");
        assert_eq!(stored["staticPublic"].as_str().unwrap().len(), 43);
        // The start body carries the key and a challenge that the poll's verifier answers.
        let seen = seen.lock().unwrap();
        let (start_path, start) = &seen[0];
        assert_eq!(start_path, "/api/pair/start");
        assert_eq!(start["machineName"], "stub-machine");
        assert_eq!(start["publicKey"], stored["staticPublic"]);
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(seen[1].1["verifier"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            b64(&Sha256::digest(verifier)),
            start["challenge"].as_str().unwrap()
        );
        assert_eq!(seen[1].1["deviceCode"], "obviously-fake-device-code");
        assert_eq!(seen.len(), 4);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(PAIRING_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "the pairing file must be private");
        }
    }

    #[test]
    fn pair_exit_4_when_denied_and_stores_nothing() {
        let (base, _) = stub(vec![(200, r#"{"status":"denied"}"#)]);
        let dir = tempfile::tempdir().unwrap();
        let (code, out, _) = pair_capture(&base, dir.path(), Duration::from_secs(20));
        assert_eq!(code, 4);
        assert!(out.contains("ABCD-2345"));
        assert!(!dir.path().join(PAIRING_FILE).exists());
    }

    #[test]
    fn pair_exit_5_when_the_server_is_unreachable() {
        // A port that was just released: nothing listens there.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let dir = tempfile::tempdir().unwrap();
        let (code, _, err) = pair_capture(
            &format!("http://127.0.0.1:{port}"),
            dir.path(),
            Duration::from_secs(5),
        );
        assert_eq!(code, 5, "{err}");
    }

    #[test]
    fn pair_times_out_with_exit_4() {
        let (base, _) = stub(vec![(200, r#"{"status":"pending"}"#); 1000]);
        let dir = tempfile::tempdir().unwrap();
        let (code, _, err) = pair_capture(&base, dir.path(), Duration::from_millis(200));
        assert_eq!(code, 4);
        assert!(err.contains("timed out"));
    }
}
