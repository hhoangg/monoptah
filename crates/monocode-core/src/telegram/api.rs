//! Blocking Telegram Bot API client and error classification.
//!
//! Every call POSTs JSON to `{base}/bot{token}/{method}`. The token is part of the URL, so
//! no error string built here may carry the URL or the token.
use serde_json::{json, Value};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Extra read time on top of the long-poll timeout so the server answers first.
const POLL_TIMEOUT_MARGIN_S: u64 = 10;
/// Wait used when Telegram sends a 429 without `parameters.retry_after`.
const DEFAULT_RETRY_AFTER_S: u64 = 5;
/// Longest wait honoured from a 429, so a bogus `retry_after` cannot overflow a deadline.
pub const MAX_RETRY_AFTER_S: u64 = 3600;

#[derive(Debug, PartialEq)]
pub enum TgError {
    /// 429 with `parameters.retry_after` (seconds).
    RetryAfter(u64),
    /// 400 with `parameters.migrate_to_chat_id`: the group became a supergroup.
    Migrated(i64),
    /// 400 "message is not modified".
    NotModified,
    /// 400 "message thread not found": the topic was deleted.
    ThreadNotFound,
    /// 400 "message to edit not found": the message was deleted (or never existed), so
    /// there is nothing left to edit.
    MessageNotFound,
    /// 400 "query is too old" / "query ID is invalid".
    QueryTooOld,
    /// 409 from getUpdates. `webhook` is true when a webhook is set on the token.
    Conflict { webhook: bool },
    /// 401: the bot token is wrong or revoked.
    Unauthorized,
    /// Other 400/403 (and unexpected 4xx) with Telegram's description.
    Permanent(String),
    /// Transport failure or 5xx; safe to retry with backoff.
    Network(String),
}

pub struct BotApi {
    base: String,
    token: String,
}

impl BotApi {
    pub fn new(base: &str, token: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    fn call(&self, method: &str, payload: Value, timeout: Duration) -> Result<Value, TgError> {
        let url = format!("{}/bot{}/{}", self.base, self.token, method);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirects(0)
            .build();
        match agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&payload.to_string())
        {
            Ok(response) => {
                let body = response.into_string().map_err(|error| {
                    TgError::Network(self.redact(&format!("Could not read response: {error}")))
                })?;
                parse_ok_body(&body)
            }
            Err(ureq::Error::Status(status, response)) => Err(classify(
                status,
                &response.into_string().unwrap_or_default(),
            )),
            Err(ureq::Error::Transport(transport)) => {
                // The transport Display includes the request URL, which carries the token.
                let detail = transport.message().unwrap_or("request failed");
                Err(TgError::Network(
                    self.redact(&format!("{}: {detail}", transport.kind())),
                ))
            }
        }
    }

    fn redact(&self, text: &str) -> String {
        if self.token.is_empty() {
            return text.to_string();
        }
        text.replace(&self.token, "<token>")
    }

    pub fn get_me(&self) -> Result<Value, TgError> {
        self.call("getMe", json!({}), REQUEST_TIMEOUT)
    }

    pub fn get_updates(&self, offset: i64, timeout_s: u64) -> Result<Vec<Value>, TgError> {
        let result = self.call(
            "getUpdates",
            json!({
                "offset": offset,
                "timeout": timeout_s,
                "allowed_updates": ["message", "callback_query"],
            }),
            Duration::from_secs(timeout_s + POLL_TIMEOUT_MARGIN_S),
        )?;
        Ok(result.as_array().cloned().unwrap_or_default())
    }

    pub fn send_message(
        &self,
        chat: i64,
        thread: Option<i64>,
        html: &str,
        keyboard: Option<Value>,
    ) -> Result<i64, TgError> {
        let mut payload = text_payload(chat, html, keyboard);
        if let Some(thread) = thread {
            payload["message_thread_id"] = json!(thread);
        }
        let result = self.call("sendMessage", payload, REQUEST_TIMEOUT)?;
        result_id(&result, "message_id")
    }

    pub fn edit_message(
        &self,
        chat: i64,
        message_id: i64,
        html: &str,
        keyboard: Option<Value>,
    ) -> Result<(), TgError> {
        let mut payload = text_payload(chat, html, keyboard);
        payload["message_id"] = json!(message_id);
        self.call("editMessageText", payload, REQUEST_TIMEOUT)
            .map(|_| ())
    }

    pub fn create_forum_topic(&self, chat: i64, name: &str) -> Result<i64, TgError> {
        let result = self.call(
            "createForumTopic",
            json!({ "chat_id": chat, "name": name }),
            REQUEST_TIMEOUT,
        )?;
        result_id(&result, "message_thread_id")
    }

    pub fn answer_callback_query(&self, id: &str, text: &str) -> Result<(), TgError> {
        self.call(
            "answerCallbackQuery",
            json!({ "callback_query_id": id, "text": text }),
            REQUEST_TIMEOUT,
        )
        .map(|_| ())
    }

    pub fn get_chat(&self, chat: i64) -> Result<Value, TgError> {
        self.call("getChat", json!({ "chat_id": chat }), REQUEST_TIMEOUT)
    }

    pub fn get_chat_member(&self, chat: i64, user: i64) -> Result<Value, TgError> {
        self.call(
            "getChatMember",
            json!({ "chat_id": chat, "user_id": user }),
            REQUEST_TIMEOUT,
        )
    }

    pub fn delete_webhook(&self) -> Result<(), TgError> {
        self.call("deleteWebhook", json!({}), REQUEST_TIMEOUT)
            .map(|_| ())
    }
}

fn text_payload(chat: i64, html: &str, keyboard: Option<Value>) -> Value {
    let mut payload = json!({
        "chat_id": chat,
        "text": html,
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    if let Some(keyboard) = keyboard {
        payload["reply_markup"] = keyboard;
    }
    payload
}

fn result_id(result: &Value, field: &str) -> Result<i64, TgError> {
    result
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| TgError::Permanent(format!("Telegram response had no {field}")))
}

/// Maps an HTTP status and error body to a `TgError`. Pure.
pub fn classify(status: u16, body: &str) -> TgError {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lowered = description.to_lowercase();
    let parameters = value.get("parameters");
    if status == 429 {
        let seconds = parameters
            .and_then(|p| p.get("retry_after"))
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_RETRY_AFTER_S)
            .min(MAX_RETRY_AFTER_S);
        return TgError::RetryAfter(seconds);
    }
    if let Some(chat) = parameters
        .and_then(|p| p.get("migrate_to_chat_id"))
        .and_then(Value::as_i64)
    {
        return TgError::Migrated(chat);
    }
    match status {
        401 => TgError::Unauthorized,
        409 => TgError::Conflict {
            webhook: lowered.contains("webhook"),
        },
        500..=599 => TgError::Network(format!("HTTP {status}")),
        _ if lowered.contains("message is not modified") => TgError::NotModified,
        _ if lowered.contains("message thread not found") => TgError::ThreadNotFound,
        _ if lowered.contains("message to edit not found") => TgError::MessageNotFound,
        _ if lowered.contains("query is too old") || lowered.contains("query id is invalid") => {
            TgError::QueryTooOld
        }
        _ if description.is_empty() => TgError::Permanent(format!("HTTP {status}")),
        _ => TgError::Permanent(description.to_string()),
    }
}

/// Unwraps `{"ok":true,"result":...}`; an `ok:false` body goes through `classify`.
fn parse_ok_body(body: &str) -> Result<Value, TgError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| TgError::Permanent("Telegram sent an unreadable response".into()))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
    let code = value
        .get("error_code")
        .and_then(Value::as_u64)
        .and_then(|code| u16::try_from(code).ok())
        .unwrap_or(400);
    Err(classify(code, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Instant;

    const TOKEN: &str = "123456:SECRET-token-value";

    /// Serves one canned response and returns the request line and body it received.
    fn serve_once(status: u16, body: Value) -> (String, thread::JoinHandle<(String, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return (String::new(), String::new());
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap();
                    }
                }
            }
            let mut request_body = vec![0u8; length];
            reader.read_exact(&mut request_body).unwrap();
            let body = body.to_string();
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            (
                request_line.trim().to_string(),
                String::from_utf8(request_body).unwrap(),
            )
        });
        (base, server)
    }

    fn error_body(code: u16, description: &str, parameters: Value) -> String {
        json!({
            "ok": false,
            "error_code": code,
            "description": description,
            "parameters": parameters,
        })
        .to_string()
    }

    #[test]
    fn classifies_retry_after() {
        let body = error_body(
            429,
            "Too Many Requests: retry after 7",
            json!({ "retry_after": 7 }),
        );
        assert_eq!(classify(429, &body), TgError::RetryAfter(7));
    }

    #[test]
    fn huge_retry_after_is_clamped() {
        let body = error_body(
            429,
            "Too Many Requests: retry after 18446744073709551615",
            json!({ "retry_after": u64::MAX }),
        );
        assert_eq!(classify(429, &body), TgError::RetryAfter(MAX_RETRY_AFTER_S));
    }

    #[test]
    fn classifies_migrate_to_chat_id() {
        let body = error_body(
            400,
            "Bad Request: group chat was upgraded to a supergroup chat",
            json!({ "migrate_to_chat_id": -100123 }),
        );
        assert_eq!(classify(400, &body), TgError::Migrated(-100123));
    }

    #[test]
    fn classifies_not_modified() {
        let body = error_body(
            400,
            "Bad Request: message is not modified: specified new message content and reply markup are exactly the same",
            json!(null),
        );
        assert_eq!(classify(400, &body), TgError::NotModified);
    }

    #[test]
    fn classifies_thread_not_found() {
        let body = error_body(400, "Bad Request: message thread not found", json!(null));
        assert_eq!(classify(400, &body), TgError::ThreadNotFound);
    }

    #[test]
    fn classifies_message_to_edit_not_found() {
        let body = error_body(400, "Bad Request: message to edit not found", json!(null));
        assert_eq!(classify(400, &body), TgError::MessageNotFound);
        // Other refusals of an edit stay permanent: only the missing message is "done".
        let other = error_body(400, "Bad Request: message can't be edited", json!(null));
        assert_eq!(
            classify(400, &other),
            TgError::Permanent("Bad Request: message can't be edited".into())
        );
        let reply = error_body(
            400,
            "Bad Request: message to be replied not found",
            json!(null),
        );
        assert_eq!(
            classify(400, &reply),
            TgError::Permanent("Bad Request: message to be replied not found".into())
        );
    }

    #[test]
    fn classifies_query_too_old() {
        let old = error_body(
            400,
            "Bad Request: query is too old and response timeout expired or query ID is invalid",
            json!(null),
        );
        assert_eq!(classify(400, &old), TgError::QueryTooOld);
        let invalid = error_body(400, "Bad Request: query ID is invalid", json!(null));
        assert_eq!(classify(400, &invalid), TgError::QueryTooOld);
    }

    #[test]
    fn classifies_conflict_webhook() {
        let body = error_body(
            409,
            "Conflict: can't use getUpdates method while webhook is active; use deleteWebhook to delete the webhook first",
            json!(null),
        );
        assert_eq!(classify(409, &body), TgError::Conflict { webhook: true });
    }

    #[test]
    fn classifies_conflict_poller() {
        let body = error_body(
            409,
            "Conflict: terminated by other getUpdates request; make sure that only one bot instance is running",
            json!(null),
        );
        assert_eq!(classify(409, &body), TgError::Conflict { webhook: false });
    }

    #[test]
    fn classifies_unauthorized_permanent_and_server_errors() {
        let unauthorized = error_body(401, "Unauthorized", json!(null));
        assert_eq!(classify(401, &unauthorized), TgError::Unauthorized);
        let forbidden = error_body(
            403,
            "Forbidden: bot was kicked from the group chat",
            json!(null),
        );
        assert_eq!(
            classify(403, &forbidden),
            TgError::Permanent("Forbidden: bot was kicked from the group chat".into())
        );
        assert_eq!(
            classify(400, "not json"),
            TgError::Permanent("HTTP 400".into())
        );
        assert_eq!(classify(502, "<html>"), TgError::Network("HTTP 502".into()));
    }

    #[test]
    fn send_message_posts_thread_and_keyboard() {
        let (base, server) = serve_once(200, json!({ "ok": true, "result": { "message_id": 42 } }));
        let api = BotApi::new(&base, TOKEN);
        let keyboard = json!({ "inline_keyboard": [[{ "text": "Yes", "callback_data": "a" }]] });
        let id = api
            .send_message(-100500, Some(9), "<b>Hi</b>", Some(keyboard.clone()))
            .expect("send should succeed");
        assert_eq!(id, 42);
        let (request_line, body) = server.join().unwrap();
        assert_eq!(
            request_line,
            format!("POST /bot{TOKEN}/sendMessage HTTP/1.1")
        );
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["chat_id"], -100500);
        assert_eq!(body["message_thread_id"], 9);
        assert_eq!(body["reply_markup"], keyboard);
        assert_eq!(body["parse_mode"], "HTML");
        assert_eq!(body["disable_web_page_preview"], true);
        assert_eq!(body["text"], "<b>Hi</b>");
    }

    #[test]
    fn send_message_omits_thread_and_keyboard_when_absent() {
        let (base, server) = serve_once(200, json!({ "ok": true, "result": { "message_id": 1 } }));
        BotApi::new(&base, TOKEN)
            .send_message(5, None, "x", None)
            .unwrap();
        let (_, body) = server.join().unwrap();
        let body: Value = serde_json::from_str(&body).unwrap();
        assert!(body.get("message_thread_id").is_none());
        assert!(body.get("reply_markup").is_none());
    }

    #[test]
    fn error_response_is_classified() {
        let (base, server) = serve_once(
            429,
            serde_json::from_str(&error_body(
                429,
                "Too Many Requests",
                json!({ "retry_after": 3 }),
            ))
            .unwrap(),
        );
        let error = BotApi::new(&base, TOKEN)
            .send_message(5, None, "x", None)
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error, TgError::RetryAfter(3));
    }

    #[test]
    fn get_updates_requests_message_and_callback_updates() {
        let (base, server) = serve_once(
            200,
            json!({ "ok": true, "result": [{ "update_id": 10 }, { "update_id": 11 }] }),
        );
        let updates = BotApi::new(&base, TOKEN).get_updates(10, 50).unwrap();
        assert_eq!(updates.len(), 2);
        let (request_line, body) = server.join().unwrap();
        assert!(request_line.contains("/getUpdates"));
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["offset"], 10);
        assert_eq!(body["timeout"], 50);
        assert_eq!(
            body["allowed_updates"],
            json!(["message", "callback_query"])
        );
    }

    #[test]
    fn network_error_hides_token() {
        // Port 1 on loopback refuses connections, so ureq reports a transport error.
        let error = BotApi::new("http://127.0.0.1:1", TOKEN)
            .get_me()
            .unwrap_err();
        match error {
            TgError::Network(message) => {
                assert!(!message.is_empty());
                assert!(!message.contains(TOKEN), "token leaked: {message}");
                assert!(!message.contains("SECRET"), "token leaked: {message}");
            }
            other => panic!("expected Network, got {other:?}"),
        }
    }
}
