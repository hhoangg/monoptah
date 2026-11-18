//! The desktop's blocking JSON client of the web server's pairing and
//! `/api/desktop/*` routes (architecture 6.4, 6.7 and 7.2).
//!
//! Every call is blocking with a 10 s timeout: callers run them on a worker
//! thread or on `spawn_blocking`, never on an async worker. The desktop token
//! is put in a `Zeroizing` header string and never appears in an error text,
//! a `Debug` output or a URL.
use super::relay::relay_url;
use crate::mobile::policy::Permission;
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;
use zeroize::Zeroizing;

const TIMEOUT: Duration = Duration::from_secs(10);
/// `DELETE /api/desktop/me` runs while the user waits for "Unpair".
pub(crate) const UNPAIR_TIMEOUT: Duration = Duration::from_secs(8);
/// A web server answer is a small JSON document.
const MAX_BODY: u64 = 1024 * 1024;
const RELAY_PATH: &str = "/relay/desktop/";

/// The HTTP base (`https://HOST[:port]`, or `http://` for a local dev host)
/// for a host URL the user typed, or for a stored relay URL (`wss://HOST` or
/// `wss://HOST/relay/desktop/<id>`). `http` and `ws` are accepted only when
/// `allow_insecure_localhost` is set and the host is local.
pub fn base_url(url: &str, allow_insecure_localhost: bool) -> Result<String, String> {
    let url = url.trim();
    let host_part = url.split_once(RELAY_PATH).map_or(url, |(head, _)| head);
    let relay = relay_url(host_part, "x", allow_insecure_localhost)?;
    let base = relay
        .strip_suffix("/relay/desktop/x")
        .ok_or("Invalid web host address")?;
    Ok(if let Some(rest) = base.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = base.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        base.to_owned()
    })
}

/// The id in a `.../relay/desktop/<id>` URL, if it has that form.
pub fn relay_path_id(url: &str) -> Option<&str> {
    url.trim().split_once(RELAY_PATH).map(|(_, id)| id)
}

/// What `POST /api/pair/start` answered.
pub struct PairStarted {
    pub user_code: String,
    pub device_code: Zeroizing<String>,
    pub confirmation_code: String,
    pub expires_in: u64,
    pub interval: u64,
}

impl std::fmt::Debug for PairStarted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairStarted")
            .field("user_code", &self.user_code)
            .field("device_code", &"<redacted>")
            .field("confirmation_code", &self.confirmation_code)
            .finish()
    }
}

/// The approval of a pairing: returned by the server exactly once.
pub struct Approval {
    pub desktop_id: String,
    pub desktop_token: Zeroizing<String>,
    pub handle: String,
    pub relay_url: String,
}

impl std::fmt::Debug for Approval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Approval")
            .field("desktop_id", &self.desktop_id)
            .field("desktop_token", &"<redacted>")
            .field("handle", &self.handle)
            .finish()
    }
}

/// One answer to `POST /api/pair/poll`.
#[derive(Debug)]
pub enum PollOutcome {
    Pending,
    /// `slow_down`, or HTTP 429 with its `Retry-After` seconds.
    SlowDown {
        retry_after: Option<u64>,
    },
    Denied,
    Expired,
    Approved(Approval),
}

/// One grant of `GET /api/desktop/grants`. `device_name` is untrusted text and
/// `fingerprint` is the server's claim: the desktop computes its own from
/// `public_key`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerGrant {
    pub device_id: String,
    pub device_name: String,
    /// Base64url X25519 public key.
    pub public_key: String,
    pub fingerprint: String,
    /// `pending`, `confirmed` or `revoked`.
    pub state: String,
    pub requested: Option<Permission>,
    pub permission: Option<Permission>,
}

pub(crate) fn permission_name(permission: Permission) -> &'static str {
    match permission {
        Permission::Read => "read",
        Permission::Control => "control",
    }
}

fn parse_permission(value: &Value) -> Option<Permission> {
    match value.as_str() {
        Some("read") => Some(Permission::Read),
        Some("control") => Some(Permission::Control),
        _ => None,
    }
}

const UNREACHABLE: &str = "Cannot reach the web server";

struct Answer {
    status: u16,
    body: Value,
    retry_after: Option<u64>,
}

/// One call. `Ok` for any HTTP answer, `Err` (with a text free of secrets)
/// when there was none. Redirects are never followed, so the bearer header
/// cannot leave the host it was meant for.
fn call(
    method: &str,
    base: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<Answer, String> {
    let mut request = ureq::AgentBuilder::new()
        .timeout(timeout)
        .redirects(0)
        .build()
        .request(method, &format!("{base}{path}"));
    if let Some(token) = token {
        let bearer = Zeroizing::new(format!("Bearer {token}"));
        request = request.set("Authorization", &bearer);
    }
    let result = match body {
        Some(body) => request
            .set("Content-Type", "application/json")
            .send_string(&body.to_string()),
        None => request.call(),
    };
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(_)) => return Err(UNREACHABLE.into()),
    };
    let status = response.status();
    let retry_after = response
        .header("Retry-After")
        .and_then(|value| value.trim().parse::<u64>().ok());
    let mut text = String::new();
    response
        .into_reader()
        .take(MAX_BODY)
        .read_to_string(&mut text)
        .map_err(|_| "The web server sent an unreadable answer".to_owned())?;
    Ok(Answer {
        status,
        body: serde_json::from_str(&text).unwrap_or(Value::Null),
        retry_after,
    })
}

fn failed(status: u16) -> String {
    format!("The web server answered {status}")
}

fn ok_body(answer: Answer) -> Result<Value, String> {
    if (200..300).contains(&answer.status) {
        Ok(answer.body)
    } else {
        Err(failed(answer.status))
    }
}

/// A server device id goes into a URL path: only the characters an id has.
fn path_id(id: &str) -> Result<&str, String> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid
        .then_some(id)
        .ok_or_else(|| "Invalid device id".into())
}

/// `POST /api/pair/start`.
pub fn pair_start(
    base: &str,
    public_key: &str,
    machine_name: &str,
    challenge: &str,
) -> Result<PairStarted, String> {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    };
    let body = json!({
        "publicKey": public_key,
        "machineName": machine_name,
        "os": os,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "challenge": challenge,
    });
    let answer = call("POST", base, "/api/pair/start", None, Some(&body), TIMEOUT)?;
    let body = ok_body(answer)?;
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
    let (Some(user_code), Some(device_code), Some(confirmation_code)) = (
        text("userCode"),
        text("deviceCode"),
        text("confirmationCode"),
    ) else {
        return Err("The web server sent an incomplete pairing".into());
    };
    Ok(PairStarted {
        user_code,
        device_code: Zeroizing::new(device_code),
        confirmation_code,
        expires_in: body["expiresIn"].as_u64().unwrap_or(600).clamp(60, 900),
        interval: body["interval"].as_u64().unwrap_or(3).clamp(1, 60),
    })
}

/// `POST /api/pair/poll`. A 429 counts as `slow_down`.
pub fn pair_poll(base: &str, device_code: &str, verifier: &str) -> Result<PollOutcome, String> {
    let body = json!({"deviceCode": device_code, "verifier": verifier});
    // A blip (no answer, or a 5xx from the edge) must not end a pairing the
    // user may just have approved: keep polling until the code expires.
    let answer = match call("POST", base, "/api/pair/poll", None, Some(&body), TIMEOUT) {
        Ok(answer) => answer,
        Err(error) if error == UNREACHABLE => return Ok(PollOutcome::Pending),
        Err(error) => return Err(error),
    };
    if answer.status >= 500 {
        return Ok(PollOutcome::Pending);
    }
    if answer.status == 429 {
        return Ok(PollOutcome::SlowDown {
            retry_after: answer.retry_after,
        });
    }
    let retry_after = answer.retry_after;
    let body = ok_body(answer)?;
    match body["status"].as_str() {
        Some("pending") => Ok(PollOutcome::Pending),
        Some("slow_down") => Ok(PollOutcome::SlowDown { retry_after }),
        Some("denied") => Ok(PollOutcome::Denied),
        Some("expired") => Ok(PollOutcome::Expired),
        Some("approved") => {
            let text = |value: &Value| value.as_str().map(str::to_owned);
            match (
                text(&body["desktopId"]),
                text(&body["desktopToken"]),
                text(&body["account"]["handle"]),
                text(&body["relayUrl"]),
            ) {
                (Some(desktop_id), Some(token), Some(handle), Some(relay_url)) => {
                    Ok(PollOutcome::Approved(Approval {
                        desktop_id,
                        desktop_token: Zeroizing::new(token),
                        handle,
                        relay_url,
                    }))
                }
                _ => Err("The web server sent an incomplete approval".into()),
            }
        }
        _ => Err("The web server sent an unknown pairing state".into()),
    }
}

/// `GET /api/desktop/grants`.
pub fn desktop_grants(base: &str, token: &str) -> Result<Vec<ServerGrant>, String> {
    let answer = call(
        "GET",
        base,
        "/api/desktop/grants",
        Some(token),
        None,
        TIMEOUT,
    )?;
    let body = ok_body(answer)?;
    let text = |value: &Value| value.as_str().map(str::to_owned);
    let grants = body["grants"]
        .as_array()
        .ok_or("The web server sent an unreadable grant list")?
        .iter()
        .filter_map(|grant| {
            Some(ServerGrant {
                device_id: text(&grant["deviceId"])?,
                device_name: text(&grant["deviceName"]).unwrap_or_default(),
                public_key: text(&grant["publicKey"])?,
                fingerprint: text(&grant["fingerprint"]).unwrap_or_default(),
                state: text(&grant["state"])?,
                requested: parse_permission(&grant["requestedPermission"]),
                permission: parse_permission(&grant["permission"]),
            })
        })
        .collect();
    Ok(grants)
}

/// `POST /api/desktop/grants/:deviceId/state`: what the desktop applied.
pub fn report_grant_state(
    base: &str,
    token: &str,
    device_id: &str,
    state: &str,
    permission: Option<Permission>,
) -> Result<(), String> {
    let mut body = json!({"state": state});
    if let Some(permission) = permission {
        body["permission"] = json!(permission_name(permission));
    }
    let path = format!("/api/desktop/grants/{}/state", path_id(device_id)?);
    ok_body(call(
        "POST",
        base,
        &path,
        Some(token),
        Some(&body),
        TIMEOUT,
    )?)
    .map(|_| ())
}

/// `DELETE /api/desktop/me`: the desktop unpairs itself.
pub fn delete_me(base: &str, token: &str, timeout: Duration) -> Result<(), String> {
    ok_body(call(
        "DELETE",
        base,
        "/api/desktop/me",
        Some(token),
        None,
        timeout,
    )?)
    .map(|_| ())
}

/// `POST /api/push/notify`. Returns how many pushes the server sent.
pub fn push_notify(base: &str, token: &str, body: &Value) -> Result<u64, String> {
    let answer = call(
        "POST",
        base,
        "/api/push/notify",
        Some(token),
        Some(body),
        TIMEOUT,
    )?;
    Ok(ok_body(answer)?["sent"].as_u64().unwrap_or(0))
}
