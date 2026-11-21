//! The two HTTP halves of the gate: a tiny loopback MCP server for the agent (Streamable HTTP,
//! JSON replies only) and the client the gate uses to reach the Playwright server.
use super::Upstream;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const LOOPBACK: &str = "127.0.0.1";
const MAX_HEADER_BYTES: u64 = 64 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 32;

/// Answers one JSON-RPC message. `None` is a notification: nothing to send back.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, route: &str, message: Value) -> Option<Value>;
    /// Whether `route` was issued; any other route is a 404.
    fn knows(&self, route: &str) -> bool;
}

/// The listening half. Dropping it stops the accept loop (a connection in flight finishes).
pub struct HttpServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
}
impl HttpServer {
    /// Serves `POST /{secret}/mcp` on a free loopback port.
    pub fn start(handler: Arc<dyn Handler>, secret: String) -> Result<Self, String> {
        let listener = TcpListener::bind((LOOPBACK, 0))
            .map_err(|_| "Could not open a port for the browser gate.".to_owned())?;
        let port = listener
            .local_addr()
            .map_err(|_| "Could not open a port for the browser gate.".to_owned())?
            .port();
        let stop = Arc::new(AtomicBool::new(false));
        let owner = Arc::clone(&stop);
        let open = Arc::new(AtomicUsize::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if owner.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    open.fetch_sub(1, Ordering::SeqCst);
                    // Say so instead of leaving the client hanging.
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let _ = respond(
                        &stream,
                        "503 Service Unavailable",
                        "Retry-After: 1\r\n",
                        None,
                    );
                    continue;
                }
                let (handler, secret, open) = (Arc::clone(&handler), secret.clone(), open.clone());
                std::thread::spawn(move || {
                    let _ = serve(stream, port, &secret, handler.as_ref());
                    open.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Ok(Self { port, stop })
    }
}
impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so its thread sees the flag.
        let _ = TcpStream::connect((LOOPBACK, self.port));
    }
}

struct Request {
    method: String,
    path: String,
    host: String,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> Result<Request, String> {
    let mut reader = BufReader::new(stream.take(MAX_HEADER_BYTES + MAX_BODY_BYTES as u64));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();
    let (mut host, mut length, mut chunked) = (String::new(), None::<usize>, false);
    let mut header_bytes = 0u64;
    loop {
        let mut header = String::new();
        let read = reader.read_line(&mut header).map_err(|e| e.to_string())?;
        header_bytes += read as u64;
        if read == 0 || header_bytes > MAX_HEADER_BYTES {
            return Err("bad headers".into());
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "host" => host = value.to_owned(),
                "content-length" => length = value.parse().ok(),
                "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
                _ => {}
            }
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).map_err(|e| e.to_string())?;
            let size = usize::from_str_radix(size.trim().split(';').next().unwrap_or(""), 16)
                .map_err(|_| "bad chunk".to_owned())?;
            if size == 0 {
                break;
            }
            if body.len() + size > MAX_BODY_BYTES {
                return Err("body too large".into());
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader
                .read_exact(&mut body[start..])
                .map_err(|e| e.to_string())?;
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).map_err(|e| e.to_string())?;
        }
    } else if let Some(length) = length {
        if length > MAX_BODY_BYTES {
            return Err("body too large".into());
        }
        body.resize(length, 0);
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    }
    Ok(Request {
        method,
        path,
        host,
        body,
    })
}

fn respond(
    mut stream: &TcpStream,
    status: &str,
    extra: &str,
    body: Option<&str>,
) -> Result<(), String> {
    let body = body.unwrap_or("");
    let content_type = if body.is_empty() {
        ""
    } else {
        "Content-Type: application/json\r\n"
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\n{content_type}{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .and_then(|_| stream.flush())
    .map_err(|e| e.to_string())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

fn serve(stream: TcpStream, port: u16, secret: &str, handler: &dyn Handler) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    let request = read_request(&stream)?;
    // Only the loopback name of this port: the check against DNS rebinding.
    if request.host != format!("{LOOPBACK}:{port}") && request.host != format!("localhost:{port}") {
        return respond(&stream, "403 Forbidden", "", None);
    }
    // `/{secret}/{route}/mcp`: the route names the app session the call belongs to.
    let path = request.path.split('?').next().unwrap_or("");
    let route = path
        .strip_prefix('/')
        .and_then(|rest| rest.strip_prefix(secret))
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.strip_suffix("/mcp"))
        .filter(|route| {
            !route.is_empty()
                && route.len() <= 40
                && route
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        });
    let secret_ok = path
        .strip_prefix('/')
        .and_then(|rest| rest.get(..secret.len()))
        .is_some_and(|given| constant_time_eq(given, secret));
    let Some(route) = route.filter(|route| secret_ok && handler.knows(route)) else {
        return respond(&stream, "404 Not Found", "", None);
    };
    if request.method != "POST" {
        // No server-initiated stream and no session to end.
        return respond(&stream, "405 Method Not Allowed", "Allow: POST\r\n", None);
    }
    let message: Value = match serde_json::from_slice(&request.body) {
        Ok(message) => message,
        Err(_) => {
            return respond(
                &stream,
                "400 Bad Request",
                "",
                Some(
                    r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#,
                ),
            )
        }
    };
    let reply = match message {
        Value::Array(batch) => {
            let replies: Vec<Value> = batch
                .into_iter()
                .filter_map(|m| handler.handle(route, m))
                .collect();
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        message => handler.handle(route, message),
    };
    match reply {
        Some(reply) => respond(&stream, "200 OK", "", Some(&reply.to_string())),
        None => respond(&stream, "202 Accepted", "", None),
    }
}

/// Reaches the Playwright server over Streamable HTTP. A reply is JSON or an event stream; the
/// stream is read until the event that answers the request.
pub struct HttpUpstream {
    agent: ureq::Agent,
}
impl HttpUpstream {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                // A page action may wait for a slow page: well above Playwright's own limits.
                .timeout_read(Duration::from_secs(300))
                .build(),
        }
    }
}
impl Default for HttpUpstream {
    fn default() -> Self {
        Self::new()
    }
}
impl Upstream for HttpUpstream {
    fn post(
        &self,
        url: &str,
        session: Option<&str>,
        message: &Value,
    ) -> Result<(Option<Value>, Option<String>), String> {
        let mut request = self
            .agent
            .post(url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream");
        if let Some(session) = session {
            request = request.set("Mcp-Session-Id", session);
        }
        let response = match request.send_string(&message.to_string()) {
            Ok(response) => response,
            Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code}")),
            Err(error) => return Err(error.to_string()),
        };
        let session = response.header("mcp-session-id").map(str::to_owned);
        let Some(id) = message.get("id").filter(|id| !id.is_null()).cloned() else {
            return Ok((None, session));
        };
        let event_stream = response
            .header("content-type")
            .is_some_and(|t| t.contains("text/event-stream"));
        if !event_stream {
            let mut text = String::new();
            response
                .into_reader()
                .take(64 * 1024 * 1024)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            let reply = serde_json::from_str(&text).map_err(|_| "Unreadable reply".to_owned())?;
            return Ok((Some(reply), session));
        }
        let mut data = String::new();
        for line in BufReader::new(response.into_reader()).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if let Some(chunk) = line.strip_prefix("data:") {
                data.push_str(chunk.trim_start());
            } else if line.is_empty() && !data.is_empty() {
                let event = std::mem::take(&mut data);
                if let Ok(reply) = serde_json::from_str::<Value>(&event) {
                    if reply["id"] == id {
                        return Ok((Some(reply), session));
                    }
                }
            }
        }
        Err("The browser server ended its reply early.".into())
    }
}
