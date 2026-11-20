//! Notifications for paired phones, sent through Expo's push service so they
//! arrive while the app is closed. The desktop sends them itself; there is no
//! server of ours in between.
use serde_json::{json, Value};
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

/// What a web notice says. By default it carries the session or Mono name and
/// a short plain preview, which the Worker encrypts to each browser and never
/// logs or stores (architecture.md section 8). With the "Show message content
/// in notifications" setting off (`policy::PolicyFlags::show_push_content`)
/// it carries neither, only enough for the browser to build a generic line.
pub struct WebNotice<'a> {
    /// `agentFinished`, `agentInput` or `monoMessages`.
    pub category: &'a str,
    /// None for a Mono's own chat, which has no session.
    pub session_id: Option<&'a str>,
    pub mono_id: Option<&'a str>,
    /// The project's display name, or the Mono's name for a Mono notice.
    pub project: &'a str,
    /// The session title or the Mono's name: the notification title.
    pub title: &'a str,
    /// The raw text to preview (reply excerpt, command asked, question).
    /// Reduced by [`super::preview`] before it leaves the desktop.
    pub preview: Option<&'a str>,
    /// Only devices with Control access get it (a result of something a
    /// Control device asked for, such as a remote update).
    pub control_only: bool,
}

/// Tells the paired web devices that take `notice.category` about it, through
/// the web server (`POST /api/push/notify`). Returns at once: the identity
/// read and the HTTP call run on one background thread per notice, with a
/// 10 s timeout and one retry (network errors, timeouts, 5xx and 429 only),
/// then the notice is dropped (never queued). Each attempt and its outcome is
/// one `[web-push]` line on stderr, metadata only.
pub fn notify_web(notice: &WebNotice) {
    let target = target_of(notice);
    let Some(host) = super::web_link::attached_host() else {
        report(notice.category, target, 0, "skipped=no host", false);
        return;
    };
    if !host.web_enabled() {
        report(notice.category, target, 0, "skipped=web link off", false);
        return;
    }
    let status = host.web_status();
    if !status.paired {
        report(notice.category, target, 0, "skipped=unpaired", false);
        return;
    }
    let show_content = super::policy::flags().show_push_content;
    let body = match plan(notice, &status.devices, show_content, |session| {
        host.catalog_session_project(session)
    }) {
        Ok(body) => body,
        Err(skip) => {
            report(
                notice.category,
                target,
                0,
                &format!("skipped={}", skip.reason()),
                true,
            );
            return;
        }
    };
    let (category, devices) = (
        notice.category.to_owned(),
        body["deviceIds"].as_array().map_or(0, Vec::len),
    );
    let _ = std::thread::Builder::new()
        .name("monocode-web-push".into())
        .spawn(move || {
            let load = || push_target(&host);
            let outcome = deliver(load, &body, RETRY_PAUSE);
            report(&category, target, devices, &outcome, true);
        });
}

/// What a notice is about, for the log: `session`, `mono` (a Mono's own
/// chat) or `habit` (a Mono's habit run).
fn target_of(notice: &WebNotice) -> &'static str {
    let has_session = notice.session_id.is_some_and(|id| !id.is_empty());
    match (has_session, notice.mono_id.is_some()) {
        (_, false) => "session",
        (false, true) => "mono",
        (true, true) => "habit",
    }
}

/// One `[web-push]` log line. `outcome` is `result=...` or `skipped=...`.
/// Metadata only: never a token, URL, title or message text.
pub fn log_line(category: &str, target: &str, devices: usize, outcome: &str) -> String {
    format!("[web-push] category={category} target={target} devices={devices} {outcome}")
}

/// Logs that the shell dropped a notice before it reached the web push.
pub fn log_suppressed(category: &str, reason: &str) {
    eprintln!("[web-push] category={category} suppressed: {reason}");
    let active = super::web_link::attached_host()
        .is_some_and(|host| host.web_enabled() && host.web_status().paired);
    audit_if(active, category, "-", 0, &format!("suppressed={reason}"));
}

/// Whether an attempt goes to the audit log: only while the web link is
/// enabled and paired (`active`), and never for the outcomes that say the web
/// push is not in use (`skipped=no host`, `skipped=web link off`,
/// `skipped=unpaired`). A desktop-only user would otherwise fill the Activity
/// log with one line per turn.
pub(crate) fn audit_worthy(active: bool, outcome: &str) -> bool {
    active
        && !matches!(
            outcome,
            "skipped=no host" | "skipped=web link off" | "skipped=unpaired"
        )
}

fn audit_if(active: bool, category: &str, target: &str, devices: usize, outcome: &str) {
    if audit_worthy(active, outcome) {
        super::audit::record_web_push(category, target, devices, outcome);
    }
}

/// Records one attempt or drop where the user can see it on every OS: stderr
/// (dev builds) and the desktop audit log shown in Settings (release builds
/// have no console on Windows or when launched from Finder). Metadata only.
fn report(category: &str, target: &str, devices: usize, outcome: &str, active: bool) {
    eprintln!("{}", log_line(category, target, devices, outcome));
    audit_if(active, category, target, devices, outcome);
}

/// The base URL and token to post to, or None when the desktop is not paired.
/// Checked before and after the identity read, and the read leaves the cached
/// facts alone, so a push in flight cannot bring back what Unpair wiped.
pub(crate) fn push_target(host: &super::MobileHost) -> Option<(String, String)> {
    if !host.web_status().paired {
        return None;
    }
    let identity = host.read_web_identity_quietly().ok().flatten()?;
    if !host.web_status().paired {
        return None;
    }
    let base =
        super::web_link::desktop_api::base_url(&identity.relay_url, cfg!(debug_assertions)).ok()?;
    Some((base, identity.desktop_token.clone()))
}

const RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

/// Why a notice produced no request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// A category the web does not take.
    Category,
    /// No confirmed web device selected the project or Mono.
    NoDevice,
}

impl Skip {
    pub fn reason(self) -> &'static str {
        match self {
            Skip::Category => "category not pushed",
            Skip::NoDevice => "no device selected",
        }
    }
}

/// The request body, or the reason nothing should be sent. A Mono's own chat
/// has no session: its body carries `monoId` only (never an empty
/// `sessionId`, which the Worker rejects with 400) and only the Mono filter of
/// a device applies. A habit run (session and Mono) reaches a device that
/// selected either the project or the Mono and sets `habitRun`. A plain
/// session follows the project only. Only the devices that selected the
/// project or Mono are listed, so the preview reaches no one else. With
/// `show_content` off, or nothing readable to show, the body has no `title`
/// or `preview` and the browser shows its generic line.
fn plan(
    notice: &WebNotice,
    devices: &[super::host::Device],
    show_content: bool,
    project_of: impl Fn(&str) -> Option<String>,
) -> Result<Value, Skip> {
    let status = match notice.category {
        "agentFinished" => "finished",
        "agentInput" => "needs_input",
        "monoMessages" => "message",
        _ => return Err(Skip::Category),
    };
    let session = notice.session_id.filter(|id| !id.is_empty());
    let project_id = session.and_then(&project_of);
    let ids: Vec<&str> = devices
        .iter()
        .filter(|device| device.kind == super::policy::DeviceKind::Web)
        .filter(|device| {
            !notice.control_only || device.permission == super::policy::Permission::Control
        })
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
            let wanted = match (session, notice.mono_id) {
                (None, Some(_)) => mono_ok,
                (Some(_), Some(_)) => project_ok || mono_ok,
                (_, None) => project_ok && session.is_some(),
            };
            wanted.then_some(id)
        })
        .collect();
    if ids.is_empty() {
        return Err(Skip::NoDevice);
    }
    let mut body = json!({
        "category": notice.category,
        "project": notice.project,
        "status": status,
        "deviceIds": ids,
    });
    if show_content {
        let text = notice
            .preview
            .map(super::preview::preview)
            .unwrap_or_default();
        let title = super::preview::title(notice.title);
        if !text.is_empty() && !title.is_empty() {
            body["title"] = json!(title);
            body["preview"] = json!(text);
        }
    }
    if let Some(session) = session {
        body["sessionId"] = json!(session);
    }
    if let Some(mono) = notice.mono_id {
        body["monoId"] = json!(mono);
        if session.is_some() {
            body["habitRun"] = json!(true);
        }
    }
    Ok(body)
}

/// Loads the identity (None means unpaired: no request), sends once and
/// retries once after `pause`, but only when the failure can pass (see
/// [`PushError::retryable`]). Returns the log outcome: `result=...` or
/// `skipped=unpaired`.
fn deliver(
    load: impl FnOnce() -> Option<(String, String)>,
    body: &Value,
    pause: std::time::Duration,
) -> String {
    use super::web_link::desktop_api::push_notify;
    let Some((base, token)) = load() else {
        return "skipped=unpaired".into();
    };
    match push_notify(&base, &token, body) {
        Ok(sent) => return format!("result=sent({sent})"),
        Err(error) if !error.retryable() => return format!("result={error}"),
        Err(_) => {}
    }
    std::thread::sleep(pause);
    match push_notify(&base, &token, body) {
        Ok(sent) => format!("result=retry ok({sent})"),
        Err(error) => format!("result=retry failed {error}"),
    }
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
            title: "Bach Tieu Thuan",
            preview: Some("Fixed the **login** bug.\n\n```sh\ncargo test\n```"),
            control_only: false,
        }
    }

    /// `plan` with message content on, as by default.
    fn plan(
        notice: &WebNotice,
        devices: &[super::super::host::Device],
        project_of: impl Fn(&str) -> Option<String>,
    ) -> Result<Value, Skip> {
        super::plan(notice, devices, true, project_of)
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
        assert!(plan(
            &notice("agentFinished", Some("s1"), None),
            &devices,
            project_p1
        )
        .is_err());
        assert!(plan(&notice("agentFinished", Some("s1"), None), &[], project_p1).is_err());
        let all = [web_device("all", None, None)];
        assert!(plan(&notice("somethingElse", Some("s1"), None), &all, project_p1).is_err());
        assert_eq!(
            plan(&notice("somethingElse", Some("s1"), None), &all, project_p1),
            Err(Skip::Category)
        );
        assert_eq!(
            plan(&notice("agentFinished", Some("s1"), None), &[], project_p1),
            Err(Skip::NoDevice)
        );
        assert_eq!(
            plan(&notice("agentFinished", None, None), &all, project_p1),
            Err(Skip::NoDevice),
            "a plain notice with no session has nowhere to go"
        );
        assert_eq!(
            plan(&notice("agentFinished", Some(""), None), &all, project_p1),
            Err(Skip::NoDevice)
        );
        assert_eq!(
            plan(&notice("agentInput", Some("s1"), None), &all, project_p1).unwrap()["status"],
            "needs_input"
        );
    }

    #[test]
    fn a_control_only_notice_skips_read_only_devices() {
        let mut control = web_device("control", None, None);
        control.permission = super::super::policy::Permission::Control;
        let reader = web_device("reader", None, None);
        let devices = [control, reader];
        let mut update = notice("monoMessages", None, Some("m1"));
        let everyone = plan(&update, &devices, project_p1).unwrap();
        assert_eq!(everyone["deviceIds"], json!(["control", "reader"]));
        update.control_only = true;
        let only = plan(&update, &devices, project_p1).unwrap();
        assert_eq!(only["deviceIds"], json!(["control"]));
        let readers = [web_device("reader", None, None)];
        assert_eq!(plan(&update, &readers, project_p1), Err(Skip::NoDevice));
    }

    #[test]
    fn a_mono_own_chat_is_sent_by_mono_with_no_session_id() {
        let devices = [
            web_device("all", None, None),
            web_device("mono", Some(&[]), Some(&["m1"])),
            web_device("other", Some(&["p1"]), Some(&["m2"])),
            web_device("none", Some(&["p1"]), Some(&[])),
        ];
        for category in ["monoMessages", "agentInput"] {
            let body = plan(&notice(category, None, Some("m1")), &devices, project_p1).unwrap();
            assert_eq!(body["deviceIds"], json!(["all", "mono"]));
            assert_eq!(body["monoId"], "m1");
            assert!(body.get("sessionId").is_none());
            assert!(body.get("habitRun").is_none());
        }
        let body = plan(
            &notice("monoMessages", None, Some("m1")),
            &devices,
            project_p1,
        )
        .unwrap();
        assert_eq!(body["status"], "message");
        assert_eq!(body["project"], "Alpha");
        // An empty session id is the same as none, and is never sent.
        let body = plan(
            &notice("monoMessages", Some(""), Some("m1")),
            &devices,
            project_p1,
        )
        .unwrap();
        assert_eq!(body["deviceIds"], json!(["all", "mono"]));
        assert!(body.get("sessionId").is_none());
        assert!(body.get("habitRun").is_none());
        // No device selected the Mono.
        assert_eq!(
            plan(
                &notice("monoMessages", None, Some("m9")),
                &[web_device("x", Some(&["p1"]), Some(&["m2"]))],
                project_p1
            ),
            Err(Skip::NoDevice)
        );
    }

    #[test]
    fn plan_ignores_phones_and_devices_without_a_server_id() {
        let mut phone = web_device("phone", None, None);
        phone.kind = super::super::policy::DeviceKind::Phone;
        let mut unconfirmed = web_device("x", None, None);
        unconfirmed.server_device_id = None;
        assert_eq!(
            plan(
                &notice("monoMessages", None, Some("m1")),
                &[phone, unconfirmed],
                project_p1
            ),
            Err(Skip::NoDevice)
        );
    }

    #[test]
    fn log_lines_carry_metadata_only() {
        assert_eq!(
            log_line("monoMessages", "mono", 1, "result=sent(1)"),
            "[web-push] category=monoMessages target=mono devices=1 result=sent(1)"
        );
        assert_eq!(target_of(&notice("monoMessages", None, Some("m1"))), "mono");
        assert_eq!(
            target_of(&notice("agentFinished", Some("s1"), Some("m1"))),
            "habit"
        );
        assert_eq!(
            target_of(&notice("agentFinished", Some("s1"), None)),
            "session"
        );
        assert_eq!(Skip::Category.reason(), "category not pushed");
        assert_eq!(Skip::NoDevice.reason(), "no device selected");
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
        let outcome = deliver(|| None, &json!({}), std::time::Duration::ZERO);
        assert_eq!(outcome, "skipped=unpaired");
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err());
    }

    #[test]
    fn deliver_posts_once_when_the_server_accepts() {
        let (base, rx) = stub(200);
        let body = json!({"deviceIds":["d1"],"habitRun":true});
        let outcome = deliver(
            || Some((base, "test-token".into())),
            &body,
            std::time::Duration::ZERO,
        );
        assert_eq!(outcome, "result=sent(1)");
        let first = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&first).unwrap(), body);
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err());
    }

    #[test]
    fn deliver_retries_exactly_once_on_failure() {
        let (base, rx) = stub(500);
        let outcome = deliver(
            || Some((base, "test-token".into())),
            &json!({"a":1}),
            std::time::Duration::ZERO,
        );
        assert_eq!(outcome, "result=retry failed http 500");
        let wait = std::time::Duration::from_secs(2);
        assert!(rx.recv_timeout(wait).is_ok());
        assert!(rx.recv_timeout(wait).is_ok());
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err());
    }

    #[test]
    fn deliver_does_not_retry_a_client_error() {
        for status in [400, 401, 403] {
            let (base, rx) = stub(status);
            let outcome = deliver(
                || Some((base, "test-token".into())),
                &json!({"a":1}),
                std::time::Duration::ZERO,
            );
            assert_eq!(outcome, format!("result=http {status}"));
            let wait = std::time::Duration::from_secs(2);
            assert!(rx.recv_timeout(wait).is_ok());
            assert!(rx
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err());
        }
    }

    #[test]
    fn deliver_retries_a_rate_limit_and_an_unreachable_server() {
        let (base, rx) = stub(429);
        let outcome = deliver(
            || Some((base, "test-token".into())),
            &json!({"a":1}),
            std::time::Duration::ZERO,
        );
        assert_eq!(outcome, "result=retry failed http 429");
        let wait = std::time::Duration::from_secs(2);
        assert!(rx.recv_timeout(wait).is_ok());
        assert!(rx.recv_timeout(wait).is_ok());
        // A server that accepts and hangs up without answering: a fast,
        // deterministic network error (no port to collide with, and no slow
        // refused connection on Windows). Retried once, so two connections.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, hangups) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                drop(stream);
                let _ = tx.send(());
            }
        });
        let outcome = deliver(
            || Some((base, "test-token".into())),
            &json!({"a":1}),
            std::time::Duration::ZERO,
        );
        assert_eq!(outcome, "result=retry failed network");
        assert!(hangups.recv_timeout(wait).is_ok());
        assert!(hangups.recv_timeout(wait).is_ok());
        assert!(hangups
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err());
    }

    #[test]
    fn only_a_paired_enabled_web_link_is_audited() {
        assert!(audit_worthy(true, "result=sent(1)"));
        assert!(audit_worthy(true, "result=http 400"));
        assert!(audit_worthy(true, "skipped=no device selected"));
        assert!(audit_worthy(true, "skipped=category not pushed"));
        assert!(audit_worthy(true, "suppressed=watching"));
        assert!(audit_worthy(true, "suppressed=not started remotely"));
        assert!(!audit_worthy(false, "result=sent(1)"));
        assert!(!audit_worthy(false, "suppressed=not started remotely"));
        for quiet in [
            "skipped=no host",
            "skipped=web link off",
            "skipped=unpaired",
        ] {
            assert!(!audit_worthy(true, quiet));
        }
    }

    #[test]
    fn one_attempt_leaves_one_audit_record_without_content() {
        let dir = tempfile::tempdir().unwrap();
        let _turn = super::super::audit::test_log(dir.path());
        report("monoMessages", "mono", 1, "result=sent(1)", true);
        audit_if(true, "agentFinished", "-", 0, "suppressed=watching");
        // Not in use: stderr only, no record.
        report("agentFinished", "session", 0, "skipped=web link off", false);
        report("agentFinished", "session", 0, "skipped=unpaired", true);
        audit_if(false, "agentFinished", "-", 0, "suppressed=watching");
        let lines = super::super::audit::read_recent(dir.path(), 10);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["event"], "webPush");
        assert_eq!(lines[1]["category"], "monoMessages");
        assert_eq!(lines[1]["target"], "mono");
        assert_eq!(lines[1]["devices"], 1);
        assert_eq!(lines[1]["outcome"], "result=sent(1)");
        assert_eq!(lines[0]["outcome"], "suppressed=watching");
        assert_eq!(lines[0]["devices"], 0);
    }

    #[test]
    fn plan_adds_the_title_and_a_plain_preview_by_default() {
        let devices = [web_device("all", None, None)];
        let body = plan(
            &notice("agentFinished", Some("s1"), None),
            &devices,
            project_p1,
        )
        .unwrap();
        assert_eq!(body["title"], "Bach Tieu Thuan");
        assert_eq!(body["preview"], "Fixed the login bug. cargo test");
        assert_eq!(body["project"], "Alpha");
    }

    #[test]
    fn plan_sends_the_old_generic_body_when_content_is_off_or_empty() {
        let devices = [web_device("all", None, None)];
        let off = super::plan(
            &notice("agentFinished", Some("s1"), None),
            &devices,
            false,
            project_p1,
        )
        .unwrap();
        assert!(off.get("title").is_none() && off.get("preview").is_none());
        assert!(!off.to_string().contains("login"));
        let mut empty = notice("agentFinished", Some("s1"), None);
        empty.preview = Some("```\n```");
        let body = plan(&empty, &devices, project_p1).unwrap();
        assert!(body.get("title").is_none() && body.get("preview").is_none());
    }

    #[test]
    fn plan_keeps_the_preview_under_the_size_cap_and_off_unselected_devices() {
        let long = "word ".repeat(2000);
        let mut big = notice("agentInput", Some("s1"), None);
        big.preview = Some(&long);
        let devices = [
            web_device("p1", Some(&["p1"]), None),
            web_device("p2", Some(&["p2"]), None),
        ];
        let body = plan(&big, &devices, project_p1).unwrap();
        assert!(body.to_string().len() < 1200, "{}", body.to_string().len());
        // The p2 device did not select the project: it is not a recipient, so
        // the Worker never gets a payload for it.
        assert_eq!(body["deviceIds"], json!(["p1"]));
    }

    #[test]
    fn plan_previews_unclipped_replies_without_fences_or_list_markers() {
        let devices = [web_device("all", None, None)];
        let mut n = notice("agentFinished", Some("s1"), None);
        for (raw, want) in [
            ("```sh\ncargo test\n```\n\nDone.", "cargo test Done."),
            ("Run:\n```sh\ncargo test\n```", "Run: cargo test"),
            ("Steps:\n- **a**\n- b", "Steps: a b"),
            ("Error: Rate limit reached", "Error: Rate limit reached"),
        ] {
            n.preview = Some(raw);
            assert_eq!(plan(&n, &devices, project_p1).unwrap()["preview"], want);
        }
    }
}
