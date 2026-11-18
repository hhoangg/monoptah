//! Notifications for paired phones, sent through Expo's push service so they
//! arrive while the app is closed. The desktop sends them itself; there is no
//! server of ours in between.
use serde_json::{Value, json};
use std::io::Read;

const ENDPOINT: &str = "https://exp.host/--/api/v2/push/send";
/// Expo takes at most this many messages per request.
const MAX_BATCH: usize = 100;
const MAX_RESPONSE_BYTES: u64 = 1 << 20;
/// The phone's Android channel for session alerts.
const CHANNEL: &str = "sessions";

pub struct Notice {
    pub title: String,
    /// Under the title on iOS, as the project a session is in.
    pub subtitle: Option<String>,
    pub body: String,
    /// What the phone opens when it is tapped.
    pub data: Value,
}

/// What a web notice says, with no title or text: only enough for the browser
/// to build its own line (architecture.md section 8).
pub struct WebNotice<'a> {
    /// `agentFinished`, `agentInput` or `monoMessages`.
    pub category: &'a str,
    /// None for a Mono's own chat, which has no session.
    pub session_id: Option<&'a str>,
    pub mono_id: Option<&'a str>,
    /// The project's display name, or the Mono's name for a Mono notice.
    pub project: &'a str,
}

/// Tells the paired web devices that take `notice.category` about it, through
/// the web server (`POST /api/push/notify`). Returns at once: the identity
/// read and the HTTP call run on one background thread per notice, with a
/// 10 s timeout and one retry, then the notice is dropped (never queued).
pub fn notify_web(notice: &WebNotice) {
    let Some(host) = super::web_link::attached_host() else {
        return;
    };
    if !host.web_enabled() {
        return;
    }
    let status = host.web_status();
    let Some(body) = plan(notice, &status.devices, |session| {
        host.catalog_session_project(session)
    }) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("monocode-web-push".into())
        .spawn(move || {
            let load = || {
                let identity = host.load_web_identity().ok().flatten()?;
                let base = super::web_link::desktop_api::base_url(
                    &identity.relay_url,
                    cfg!(debug_assertions),
                )
                .ok()?;
                Some((base, identity.desktop_token.clone()))
            };
            deliver(load, &body, RETRY_PAUSE);
        });
}

const RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

/// The request body, or None when nothing should be sent: a category the web
/// does not take, no confirmed device that selected the project or Mono, or
/// no session id. The Worker requires `sessionId` to match `SAFE_ID`
/// (1 to 128 characters, web/worker/src/modules/push/payload.ts) and answers
/// 400 for "", so a notice with no session (a Mono's own chat) is skipped
/// rather than sent with an empty id.
fn plan(
    notice: &WebNotice,
    devices: &[super::host::Device],
    project_of: impl Fn(&str) -> Option<String>,
) -> Option<Value> {
    let status = match notice.category {
        "agentFinished" => "finished",
        "agentInput" => "needs_input",
        "monoMessages" => "message",
        _ => return None,
    };
    let session = notice.session_id.filter(|id| !id.is_empty())?;
    let project_id = project_of(session);
    let ids: Vec<&str> = devices
        .iter()
        .filter(|device| device.kind == super::policy::DeviceKind::Web)
        .filter_map(|device| {
            let id = device.server_device_id.as_deref()?;
            let project_ok = match &device.projects {
                None => true,
                Some(list) => project_id.as_ref().is_some_and(|p| list.contains(p)),
            };
            let mono_ok = notice.mono_id.is_some_and(|mono| match &device.monos {
                None => true,
                Some(list) => list.iter().any(|m| m == mono),
            });
            // A habit run (session and Mono) reaches a device that selected
            // either one; a plain session notice follows the project only.
            let wanted = if notice.mono_id.is_some() {
                project_ok || mono_ok
            } else {
                project_ok
            };
            wanted.then_some(id)
        })
        .collect();
    if ids.is_empty() {
        return None;
    }
    let mut body = json!({
        "category": notice.category,
        "project": notice.project,
        "status": status,
        "sessionId": session,
        "deviceIds": ids,
    });
    if let Some(mono) = notice.mono_id {
        body["monoId"] = json!(mono);
        body["habitRun"] = json!(true);
    }
    Some(body)
}

/// Loads the identity (None means unpaired: no request), sends once and
/// retries once after `pause`.
fn deliver(
    load: impl FnOnce() -> Option<(String, String)>,
    body: &Value,
    pause: std::time::Duration,
) {
    let Some((base, token)) = load() else {
        return;
    };
    if super::web_link::desktop_api::push_notify(&base, &token, body).is_ok() {
        return;
    }
    std::thread::sleep(pause);
    let _ = super::web_link::desktop_api::push_notify(&base, &token, body);
}

/// Sends `notice` to each token. Returns the tokens Expo says no longer reach
/// a phone, to forget.
pub fn send(tokens: &[String], notice: &Notice) -> Result<Vec<String>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(15))
        .build();
    let mut gone = Vec::new();
    for batch in tokens.chunks(MAX_BATCH) {
        let messages: Vec<Value> = batch.iter().map(|to| message(to, notice)).collect();
        let response = agent
            .post(ENDPOINT)
            .set("Accept", "application/json")
            .set("Content-Type", "application/json")
            .send_string(&Value::from(messages).to_string());
        let response = match response {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(error) => return Err(format!("Could not reach Expo push: {error}")),
        };
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        if status != 200 {
            return Err(format!("Expo push returned HTTP {status}: {value}"));
        }
        gone.extend(unregistered(batch, &value));
    }
    Ok(gone)
}

fn message(to: &str, notice: &Notice) -> Value {
    let mut message = json!({
        "to": to,
        "title": notice.title,
        "body": notice.body,
        "data": notice.data,
        "sound": "default",
        "priority": "high",
        "channelId": CHANNEL,
    });
    if let Some(subtitle) = &notice.subtitle {
        message["subtitle"] = json!(subtitle);
    }
    message
}

/// Expo answers each message in order with a ticket; `DeviceNotRegistered`
/// means the app is gone from that phone.
fn unregistered(tokens: &[String], response: &Value) -> Vec<String> {
    let Some(tickets) = response["data"].as_array() else {
        return Vec::new();
    };
    tokens
        .iter()
        .zip(tickets)
        .filter(|(_, ticket)| ticket["details"]["error"] == "DeviceNotRegistered")
        .map(|(token, _)| token.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_carry_the_tap_target_and_unregistered_phones_are_found_in_order() {
        let notice = Notice {
            title: "Fix the build".into(),
            subtitle: Some("monocode".into()),
            body: "Finished".into(),
            data: json!({"sessionId":"s"}),
        };
        let message = message("ExponentPushToken[a]", &notice);
        assert_eq!(message["data"]["sessionId"], "s");
        assert_eq!(message["subtitle"], "monocode");
        let tokens = vec!["ExponentPushToken[a]".into(), "ExponentPushToken[b]".into()];
        let response = json!({"data":[
            {"status":"ok","id":"1"},
            {"status":"error","details":{"error":"DeviceNotRegistered"}},
        ]});
        assert_eq!(unregistered(&tokens, &response), ["ExponentPushToken[b]"]);
    }

    fn web_device(
        id: &str,
        projects: Option<&[&str]>,
        monos: Option<&[&str]>,
    ) -> super::super::host::Device {
        let list = |l: Option<&[&str]>| l.map(|l| l.iter().map(|s| s.to_string()).collect());
        super::super::host::Device {
            id: format!("row-{id}"),
            name: "Browser".into(),
            fingerprint: "f".repeat(64),
            push: None,
            kind: super::super::policy::DeviceKind::Web,
            permission: super::super::policy::Permission::Read,
            projects: list(projects),
            monos: list(monos),
            server_device_id: Some(id.into()),
            created_at: None,
            last_seen: None,
        }
    }

    fn notice<'a>(
        category: &'a str,
        session: Option<&'a str>,
        mono: Option<&'a str>,
    ) -> WebNotice<'a> {
        WebNotice {
            category,
            session_id: session,
            mono_id: mono,
            project: "Alpha",
        }
    }

    fn project_p1(session: &str) -> Option<String> {
        (session == "s1").then(|| "p1".to_owned())
    }

    #[test]
    fn plan_selects_devices_by_project_and_sets_habit_run_for_mono_sessions() {
        let devices = [
            web_device("all", None, None),
            web_device("p1", Some(&["p1"]), Some(&[])),
            web_device("p2", Some(&["p2"]), Some(&[])),
            web_device("mono", Some(&[]), Some(&["m1"])),
        ];
        let body = plan(
            &notice("agentFinished", Some("s1"), None),
            &devices,
            project_p1,
        )
        .unwrap();
        assert_eq!(body["deviceIds"], json!(["all", "p1"]));
        assert_eq!(body["status"], "finished");
        assert_eq!(body["sessionId"], "s1");
        assert!(body.get("habitRun").is_none());
        let body = plan(
            &notice("monoMessages", Some("s1"), Some("m1")),
            &devices,
            project_p1,
        )
        .unwrap();
        assert_eq!(body["deviceIds"], json!(["all", "p1", "mono"]));
        assert_eq!(body["habitRun"], true);
        assert_eq!(body["monoId"], "m1");
        assert_eq!(body["status"], "message");
    }

    #[test]
    fn plan_sends_nothing_for_unselected_projects_other_categories_or_no_session() {
        let devices = [web_device("p2", Some(&["p2"]), None)];
        assert!(
            plan(
                &notice("agentFinished", Some("s1"), None),
                &devices,
                project_p1
            )
            .is_none()
        );
        assert!(plan(&notice("agentFinished", Some("s1"), None), &[], project_p1).is_none());
        let all = [web_device("all", None, None)];
        assert!(plan(&notice("somethingElse", Some("s1"), None), &all, project_p1).is_none());
        assert!(plan(&notice("agentInput", None, Some("m1")), &all, project_p1).is_none());
        assert_eq!(
            plan(&notice("agentInput", Some("s1"), None), &all, project_p1).unwrap()["status"],
            "needs_input"
        );
    }

    /// A one-thread HTTP stub: answers each request with `status`, records the bodies.
    fn stub(status: u16) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut length = 0usize;
                let mut line = String::new();
                loop {
                    line.clear();
                    if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = std::io::Read::read_exact(&mut reader, &mut body);
                let _ = tx.send(String::from_utf8_lossy(&body).into_owned());
                let answer = if status == 200 { r#"{"sent":1}"# } else { "{}" };
                let _ = std::io::Write::write_all(
                    &mut stream,
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                        answer.len()
                    )
                    .as_bytes(),
                );
            }
        });
        (base, rx)
    }

    #[test]
    fn deliver_sends_nothing_when_unpaired() {
        let (_base, rx) = stub(200);
        deliver(|| None, &json!({}), std::time::Duration::ZERO);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
    }

    #[test]
    fn deliver_posts_once_when_the_server_accepts() {
        let (base, rx) = stub(200);
        let body = json!({"deviceIds":["d1"],"habitRun":true});
        deliver(
            || Some((base, "test-token".into())),
            &body,
            std::time::Duration::ZERO,
        );
        let first = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&first).unwrap(), body);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
    }

    #[test]
    fn deliver_retries_exactly_once_on_failure() {
        let (base, rx) = stub(500);
        deliver(
            || Some((base, "test-token".into())),
            &json!({"a":1}),
            std::time::Duration::ZERO,
        );
        let wait = std::time::Duration::from_secs(2);
        assert!(rx.recv_timeout(wait).is_ok());
        assert!(rx.recv_timeout(wait).is_ok());
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
    }
}
