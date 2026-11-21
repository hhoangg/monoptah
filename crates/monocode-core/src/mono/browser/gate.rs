//! The Browser gate: a Monoptah MCP server that stands between the agent and the Mono's
//! Playwright server. The agent is only given the gate's URL, so every browser tool call passes
//! through [`Policy::evaluate`] before it is forwarded. It works in every permission mode (Auto
//! and Full Access included) because it is Monoptah code, not a harness ask mode.
//!
//! What is enforced here, and what is not (see also [`super::BrowserTrust`]):
//!
//! * Mechanical, whatever the agent does: tools outside the trust level are neither listed nor
//!   run; `browser_evaluate` and `browser_run_code_unsafe` run only after the user approves a card
//!   with the code and the agent's `explanation`; page-changing tools on a finance or blocked
//!   origin, and typed card numbers, need approval; `browser_route`, `browser_unroute` and
//!   `browser_resize` never run. Anything the user does not answer, or that cannot be asked
//!   (no channel, plan or read-only session, timeout), is denied: the gate fails closed.
//! * By the agent's cooperation: payments, purchases, subscriptions, transfers, sending, posting
//!   and commenting can not be recognised from a click. The gate offers the agent
//!   `browser_request_approval` for them and the Mono's rules require it, but nothing stops an
//!   agent that skips it. The page URL the gate checks is the last `Page URL` a tool reported.
//! * The Playwright server has no port: the app starts it as a child process and the gate talks to
//!   it over stdin and stdout ([`StdioUpstream`]), so no other process, the agent's shell
//!   included, can reach it and skip the gate. The agent can still run its own browser tooling
//!   outside the harness's MCP list; that is outside what this gate controls.
//! * One gate serves all sessions of a Mono and they share one Playwright session. Cards,
//!   denials and the remembered page are kept per route (one per app session): a session's cards
//!   show only in that session, and one session ending does not deny another's.
//!
//! An approval is shown through the app's existing approval card (desktop, web app, phone): the
//! gate sends an [`Ask`] to the run that is attached ([`Gate::attach`]), the provider loop turns it
//! into an `Event::Approval` and answers it from the user's decision.
use super::BrowserTrust;
use super::{ALWAYS_BLOCKED_TOOLS, CODE_TOOLS, KNOWN_TOOLS, READ_ONLY_TOOLS};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

mod http;
mod stdio;
pub use http::HttpUpstream;
pub use stdio::StdioUpstream;

/// A stand-in route name for code that builds a [`Runtime`](super::Runtime) without a gate.
pub const SHARED_ROUTE: &str = "shared";
/// The tool the gate adds for the actions it can not detect itself.
pub const APPROVAL_TOOL: &str = "browser_request_approval";
/// How long the gate waits for the user. Codex is told to wait a minute longer.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_CODE_CHARS: usize = 20_000;
const MAX_FIELD_CHARS: usize = 2_000;

// ---- What needs approval ----

/// What the user is asked to allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// A payment, purchase, subscription, transfer, or card or bank details. Agent-chosen.
    Payment,
    /// Sending a message, email or content to someone. Agent-chosen.
    SendMessage,
    /// Posting, commenting, reviewing or publishing. Agent-chosen.
    PostPublish,
    /// Any other risky action. Agent-chosen.
    Other,
    /// Running code in the page or the server. Chosen by the gate.
    Code,
    /// A page-changing action on a finance or blocked origin. Chosen by the gate.
    FinancePage,
    /// A card number typed into a page. Chosen by the gate.
    CardData,
}
impl Category {
    /// The values `browser_request_approval` accepts.
    pub const AGENT_CHOICES: [&'static str; 4] =
        ["payment", "send_message", "post_publish", "other"];
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "payment" => Self::Payment,
            "send_message" => Self::SendMessage,
            "post_publish" => Self::PostPublish,
            "other" => Self::Other,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Payment => "Money: payment, purchase, subscription or transfer",
            Self::SendMessage => "Sending a message",
            Self::PostPublish => "Posting, commenting or publishing",
            Self::Other => "Risky action",
            Self::Code => "Running code",
            Self::FinancePage => "Change on a bank or payment page",
            Self::CardData => "Card number typed into a page",
        }
    }
}

/// One approval the user is asked for. Everything the user needs to decide is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub category: Category,
    /// What the browser is about to do, in a sentence.
    pub action: String,
    pub url: String,
    pub details: String,
    /// The code, for [`Category::Code`].
    pub code: Option<String>,
    /// The agent's reason for running the code.
    pub explanation: String,
}
impl ApprovalRequest {
    /// The title of the approval card.
    pub fn title(&self) -> String {
        match self.category {
            Category::Code => "Allow the browser to run code?",
            Category::Payment => "Allow the browser to make a payment?",
            Category::SendMessage => "Allow the browser to send this?",
            Category::PostPublish => "Allow the browser to post this?",
            Category::FinancePage => "Allow a change on this bank or payment page?",
            Category::CardData => "Allow typing a card number into this page?",
            Category::Other => "Allow this risky browser action?",
        }
        .to_owned()
    }
    /// The body of the approval card: category, action, page, details, and for code the
    /// explanation and the code itself.
    pub fn detail(&self) -> String {
        let mut out = format!("{}\nAction: {}", self.category.label(), self.action);
        if !self.url.is_empty() {
            out.push_str(&format!("\nPage: {}", self.url));
        }
        if !self.details.is_empty() {
            out.push_str(&format!("\nDetails: {}", self.details));
        }
        if let Some(code) = &self.code {
            out.push_str(&format!(
                "\n\nWhy: {}\n\nCode:\n{code}",
                if self.explanation.is_empty() {
                    "(none given)"
                } else {
                    &self.explanation
                }
            ));
        }
        out
    }
}

/// What the gate does with one tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Refused, with the sentence the agent is told.
    Deny(String),
    Ask(ApprovalRequest),
}

/// The rules of one Mono's gate. Plain data: tests build it directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub trust: BrowserTrust,
    /// The Mono's blocked origins: a page change there needs approval, as on a finance page.
    pub blocked_origins: Vec<String>,
}
impl Policy {
    pub fn new(trust: BrowserTrust, blocked_origins: Vec<String>) -> Self {
        Self {
            trust,
            blocked_origins,
        }
    }

    /// The decision for one `tools/call`. `page_url` is the last page the tools reported.
    pub fn evaluate(&self, tool: &str, args: &Value, page_url: &str) -> Verdict {
        if ALWAYS_BLOCKED_TOOLS.contains(&tool) {
            return Verdict::Deny(format!("{tool} is never available in this browser."));
        }
        if tool == APPROVAL_TOOL {
            return self.approval_tool(args, page_url);
        }
        if !KNOWN_TOOLS.contains(&tool) {
            return Verdict::Deny(format!("{tool} is not an available browser tool."));
        }
        if let Some(reason) = unsafe_address(tool, args) {
            return Verdict::Deny(reason);
        }
        if self.trust == BrowserTrust::ReadOnly {
            let action = args["action"].as_str().unwrap_or("");
            // Closing a tab ends something the user may still need.
            let allowed = READ_ONLY_TOOLS.contains(&tool)
                && (tool != "browser_tabs" || matches!(action, "list" | "new" | "select"));
            return if allowed {
                Verdict::Allow
            } else {
                Verdict::Deny(format!(
                    "{tool} is not available: this browser is at Level 1 (read only). Tell the user what to do, or that the browser level must be raised."
                ))
            };
        }
        if CODE_TOOLS.contains(&tool) {
            return self.code(tool, args, page_url);
        }
        if READ_ONLY_TOOLS.contains(&tool) {
            return Verdict::Allow;
        }
        // A tool that changes a page, at Level 2 or 3.
        if is_finance_url(page_url, &self.blocked_origins) {
            return Verdict::Ask(ApprovalRequest {
                category: Category::FinancePage,
                action: format!("{tool} {}", short_args(args)),
                url: page_url.to_owned(),
                details: "This page is a bank, wallet or payment page.".into(),
                code: None,
                explanation: String::new(),
            });
        }
        if contains_card_number(args) {
            return Verdict::Ask(ApprovalRequest {
                category: Category::CardData,
                action: format!("{tool} with what looks like a payment card number"),
                url: page_url.to_owned(),
                details: String::new(),
                code: None,
                explanation: String::new(),
            });
        }
        Verdict::Allow
    }

    fn approval_tool(&self, args: &Value, page_url: &str) -> Verdict {
        if self.trust == BrowserTrust::ReadOnly {
            return Verdict::Deny(
                "At Level 1 the browser can not act, so there is nothing to approve.".into(),
            );
        }
        let text = |key: &str| clip(args[key].as_str().unwrap_or("").trim(), MAX_FIELD_CHARS);
        let Some(category) = args["category"].as_str().and_then(Category::parse) else {
            return Verdict::Deny(format!(
                "Give a category: one of {}.",
                Category::AGENT_CHOICES.join(", ")
            ));
        };
        let action = text("action");
        if action.is_empty() {
            return Verdict::Deny("Describe the action in `action`.".into());
        }
        let url = text("url");
        Verdict::Ask(ApprovalRequest {
            category,
            action,
            url: if url.is_empty() {
                page_url.to_owned()
            } else {
                url
            },
            details: text("details"),
            code: None,
            explanation: String::new(),
        })
    }

    fn code(&self, tool: &str, args: &Value, page_url: &str) -> Verdict {
        let explanation = clip(
            args["explanation"].as_str().unwrap_or("").trim(),
            MAX_FIELD_CHARS,
        );
        if explanation.is_empty() {
            return Verdict::Deny(format!(
                "{tool} needs an `explanation` argument that says why the code must run. The user sees it next to the code."
            ));
        }
        let code = if tool == "browser_evaluate" {
            args["function"].as_str()
        } else {
            // `filename` would load code the user is never shown.
            if args.get("filename").is_some_and(|f| !f.is_null()) {
                return Verdict::Deny(
                    "browser_run_code_unsafe takes `code`, not `filename`: the user must see the code.".into(),
                );
            }
            args["code"].as_str()
        };
        let Some(code) = code.map(str::trim).filter(|c| !c.is_empty()) else {
            return Verdict::Deny(format!("{tool} needs the code to run."));
        };
        if code.chars().count() > MAX_CODE_CHARS {
            return Verdict::Deny(format!(
                "The code is longer than {MAX_CODE_CHARS} characters, too long for the user to review. Use shorter code or the normal tools."
            ));
        }
        // Always a card, at Level 2 and Level 3 alike. There is no safe "read-only code" test: a
        // text check is bypassed by `el['cli'+'ck']()` and the like, so none is attempted.
        Verdict::Ask(ApprovalRequest {
            category: Category::Code,
            action: if tool == "browser_evaluate" {
                "Run JavaScript in the page".into()
            } else {
                "Run Playwright code in the browser server".into()
            },
            url: page_url.to_owned(),
            details: String::new(),
            code: Some(code.to_owned()),
            explanation,
        })
    }

    /// Whether the gate lists `tool` to the agent.
    pub fn offers(&self, tool: &str) -> bool {
        self.trust.offers(tool)
    }
}

/// `browser_navigate` and a new tab open http and https addresses only, at every level:
/// `javascript:`, `data:`, `file:` and the rest are refused.
fn unsafe_address(tool: &str, args: &Value) -> Option<String> {
    let target = match tool {
        "browser_navigate" => args["url"].as_str().unwrap_or(""),
        "browser_tabs" if args["action"] == "new" => args["url"].as_str().unwrap_or(""),
        _ => return None,
    };
    if target.is_empty() && tool == "browser_tabs" {
        return None;
    }
    let ok = url::Url::parse(target.trim())
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
    (!ok).then(|| {
        "Only http and https addresses can be opened (not javascript:, data:, file: or others)."
            .to_owned()
    })
}

/// The address a call is about to open, if it opens one.
fn opened_address(tool: &str, args: &Value) -> Option<String> {
    let target = match tool {
        "browser_navigate" => args["url"].as_str(),
        "browser_tabs" if args["action"] == "new" => args["url"].as_str(),
        _ => None,
    }?;
    Some(target.trim().to_owned()).filter(|t| !t.is_empty())
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
fn short_args(args: &Value) -> String {
    let text = args
        .get("element")
        .or_else(|| args.get("target"))
        .and_then(Value::as_str)
        .unwrap_or("");
    clip(text, 120)
}

/// Words in a host that mark a bank, wallet or payment site. A match only asks for approval, so
/// a false positive costs one card.
const FINANCE_HOST_PARTS: &[&str] = &[
    "paypal",
    "stripe",
    "venmo",
    "revolut",
    "wise.com",
    "payoneer",
    "skrill",
    "klarna",
    "affirm",
    "coinbase",
    "binance",
    "kraken",
    "bank",
    "wallet",
    "momo.vn",
    "zalopay",
    "vnpay",
    "shopeepay",
    "vietcombank",
    "vietinbank",
    "techcombank",
    "bidv",
    "mbbank",
    "vpbank",
    "tpbank",
    "sacombank",
    "agribank",
    "chase.com",
    "wellsfargo",
    "citi.com",
    "americanexpress",
    "capitalone",
    "schwab",
    "robinhood",
    "cash.app",
    "pay.",
];
const FINANCE_PATH_PARTS: &[&str] = &[
    "checkout",
    "payment",
    "payments",
    "pay",
    "billing",
    "subscribe",
    "wallet",
    "transfer",
];

/// Whether a page is a bank, wallet or payment page, or one of the Mono's blocked origins.
pub fn is_finance_url(url: &str, blocked_origins: &[String]) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let origin = parsed.origin().ascii_serialization();
    if blocked_origins.iter().any(|o| o == &origin) {
        return true;
    }
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    if FINANCE_HOST_PARTS.iter().any(|part| host.contains(part)) {
        return true;
    }
    parsed
        .path_segments()
        .into_iter()
        .flatten()
        .any(|segment| FINANCE_PATH_PARTS.contains(&segment.to_lowercase().as_str()))
}

/// Whether any text in the arguments is a payment card number: 13 to 19 digits (spaces and
/// dashes allowed) that pass the Luhn check.
pub fn contains_card_number(args: &Value) -> bool {
    fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
        match value {
            Value::String(text) => out.push(text),
            Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
            Value::Object(map) => map.values().for_each(|v| strings(v, out)),
            _ => {}
        }
    }
    let mut texts = Vec::new();
    strings(args, &mut texts);
    texts.into_iter().any(|text| {
        let mut run = String::new();
        let mut found = false;
        for c in text.chars().chain(std::iter::once('\n')) {
            if c.is_ascii_digit() {
                run.push(c);
            } else if matches!(c, ' ' | '-') && !run.is_empty() {
                // Separators inside a number do not end the run.
            } else {
                found |= (13..=19).contains(&run.len()) && luhn(&run);
                run.clear();
            }
        }
        found
    })
}
fn luhn(digits: &str) -> bool {
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let d = u32::from(b - b'0');
            if i % 2 == 1 {
                let d = d * 2;
                if d > 9 {
                    d - 9
                } else {
                    d
                }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

// ---- Asking the user ----

/// One approval request on its way to the run that shows the card.
pub struct Ask {
    pub request: ApprovalRequest,
    /// `true` is Allow. A dropped sender is a deny.
    pub reply: mpsc::Sender<bool>,
    /// Set by the gate when it stopped waiting (timeout): the card should be withdrawn.
    pub abandoned: Arc<AtomicBool>,
}

/// A run's end of the approval channel. Dropping it detaches the run from the gate.
pub struct Attached {
    pub receiver: mpsc::Receiver<Ask>,
    gate: Arc<Inner>,
    route: String,
    generation: u64,
}
impl Attached {
    pub fn try_recv(&self) -> Result<Ask, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for Attached {
    fn drop(&mut self) {
        let mut map = self
            .gate
            .approvers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if map
            .get(&self.route)
            .is_some_and(|a| a.generation == self.generation)
        {
            map.remove(&self.route);
        }
    }
}
struct Approver {
    sender: mpsc::Sender<Ask>,
    generation: u64,
}

// ---- The upstream server ----

/// Posts one JSON-RPC message to the Playwright server. Tests replace it.
pub trait Upstream: Send + Sync {
    /// Returns the reply (for a request) and the `Mcp-Session-Id` the server named, if any.
    /// A server that refused the request answers `Err("HTTP <status>")`.
    fn post(
        &self,
        url: &str,
        session: Option<&str>,
        message: &Value,
    ) -> Result<(Option<Value>, Option<String>), String>;
    /// Whether the MCP session is still the one to use after `error`. HTTP drops it (a new
    /// handshake is cheap). A stdio server has exactly one session for its life: it closes itself
    /// on a timeout or a broken pipe, so the app starts a new server, and the gate a new session
    /// on it, at the next run.
    fn session_survives(&self, _error: &str) -> bool {
        false
    }
}

#[derive(Clone)]
struct Session {
    /// Identifies the server instance the session was opened on.
    key: String,
    id: Option<String>,
}
struct State {
    policy: Policy,
    upstream: Arc<dyn Upstream>,
    /// Names the running server (its URL, or its process for a stdio server).
    upstream_key: String,
    session: Option<Session>,
    /// The page the browser is on, as the last tool result or opened address said. The browser
    /// (and its current tab) is shared by every session of the Mono, so the finance check reads
    /// this one value whichever session writes.
    page: String,
    /// Issued routes: an unguessable token per app session (`None`: no session).
    routes: std::collections::HashMap<Option<i64>, String>,
}

/// One gate serves every session of a Mono. The Playwright side is one shared MCP session (and
/// one browser connection) per Mono. Approval cards and denials are kept per route (one per app session), so a card of one session never shows in another.
struct Inner {
    state: Mutex<State>,
    init: Mutex<()>,
    approvers: Mutex<std::collections::HashMap<String, Approver>>,
    generation: AtomicU64,
    next_id: AtomicU64,
    timeout: Duration,
}

/// One Mono's gate: its loopback server and its policy.
pub struct Gate {
    inner: Arc<Inner>,
    port: u16,
    secret: String,
    _server: http::HttpServer,
}
impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gate(port {})", self.port)
    }
}

impl Gate {
    /// Starts the gate's server in front of the Playwright server named `upstream_key`.
    pub fn start(
        upstream: Arc<dyn Upstream>,
        policy: Policy,
        upstream_key: &str,
    ) -> Result<Arc<Self>, String> {
        Self::start_with_timeout(upstream, policy, upstream_key, APPROVAL_TIMEOUT)
    }
    pub fn start_with_timeout(
        upstream: Arc<dyn Upstream>,
        policy: Policy,
        upstream_key: &str,
        timeout: Duration,
    ) -> Result<Arc<Self>, String> {
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                policy,
                upstream,
                upstream_key: upstream_key.to_owned(),
                session: None,
                page: String::new(),
                routes: Default::default(),
            }),
            init: Mutex::new(()),
            approvers: Mutex::new(Default::default()),
            generation: AtomicU64::new(0),
            next_id: AtomicU64::new(100),
            timeout,
        });
        let secret = format!("{:032x}", rand::random::<u128>());
        let server = http::HttpServer::start(inner.clone(), secret.clone())?;
        Ok(Arc::new(Self {
            port: server.port,
            inner,
            secret,
            _server: server,
        }))
    }

    /// The URL a session connects to: the path holds the gate's secret and the session's route.
    /// Cards, denials and the remembered page are kept per route.
    pub fn url_for(&self, route: &str) -> String {
        format!("http://127.0.0.1:{}/{}/{route}/mcp", self.port, self.secret)
    }
    /// The route token of an app session (`None`: a caller without one). It is random, issued on
    /// first use and stable for the gate's life, so a warm agent process keeps a working URL; the
    /// server answers only routes it issued.
    pub fn route_for(&self, session: Option<i64>) -> String {
        self.inner
            .lock_state()
            .routes
            .entry(session)
            .or_insert_with(|| format!("{:032x}", rand::random::<u128>()))
            .clone()
    }
    /// The URL of the route used by callers that have no session.
    pub fn url(&self) -> String {
        self.url_for(&self.route_for(None))
    }

    /// Applies a new policy to the calls that follow, keeping the server.
    pub fn set_policy(&self, policy: Policy) {
        self.inner.lock_state().policy = policy;
    }

    /// Applies the Mono's current trust level and blocked origins, and which Playwright server
    /// is behind the gate (a restart gives a new key). Takes effect on the next call.
    pub fn configure(&self, policy: Policy, upstream_key: &str, upstream: Arc<dyn Upstream>) {
        let mut state = self.inner.lock_state();
        state.policy = policy;
        if state.upstream_key != upstream_key {
            state.upstream_key = upstream_key.to_owned();
            state.upstream = upstream;
            state.session = None;
            state.page.clear();
        }
    }

    /// Routes the approval requests of `route` to the calling run until the result is dropped.
    /// A later attach to the same route replaces this one; other routes are untouched.
    pub fn attach(&self, route: &str) -> Attached {
        let (sender, receiver) = mpsc::channel();
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner
            .approvers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(route.to_owned(), Approver { sender, generation });
        Attached {
            receiver,
            gate: Arc::clone(&self.inner),
            route: route.to_owned(),
            generation,
        }
    }

    /// Answers one JSON-RPC message from the agent on `route` (what the HTTP server calls).
    pub fn handle(&self, route: &str, message: Value) -> Option<Value> {
        self.inner.respond(route, message)
    }
}

impl http::Handler for Inner {
    fn handle(&self, route: &str, message: Value) -> Option<Value> {
        self.respond(route, message)
    }
    fn knows(&self, route: &str) -> bool {
        self.lock_state().routes.values().any(|r| r == route)
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
fn failure(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
fn tool_text(text: impl Into<String>, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text.into()}], "isError": is_error})
}

impl Inner {
    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn respond(&self, route: &str, message: Value) -> Option<Value> {
        let id = message.get("id").filter(|id| !id.is_null())?.clone();
        let params = &message["params"];
        Some(match message["method"].as_str().unwrap_or("") {
            "initialize" => reply(
                &id,
                json!({
                    "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-03-26"),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "monoptah-browser-gate", "version": env!("CARGO_PKG_VERSION")},
                }),
            ),
            "ping" => reply(&id, json!({})),
            "tools/list" => match self.list_tools() {
                Ok(tools) => reply(&id, json!({"tools": tools})),
                Err(error) => failure(&id, -32603, &error),
            },
            "tools/call" => reply(
                &id,
                self.call_tool(
                    route,
                    params["name"].as_str().unwrap_or(""),
                    params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                ),
            ),
            _ => failure(&id, -32601, "Method not found"),
        })
    }

    /// The upstream tools this level offers (code tools get an `explanation` argument), plus the
    /// approval tool where there is something to approve.
    fn list_tools(&self) -> Result<Vec<Value>, String> {
        let policy = self.lock_state().policy.clone();
        let upstream = self.upstream_rpc("tools/list", json!({}))?;
        let mut tools: Vec<Value> = upstream["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|tool| tool["name"].as_str().is_some_and(|n| policy.offers(n)))
            .cloned()
            .collect();
        for tool in &mut tools {
            if tool["name"]
                .as_str()
                .is_some_and(|n| CODE_TOOLS.contains(&n))
            {
                tool["inputSchema"]["properties"]["explanation"] = json!({
                    "type": "string",
                    "description": "Why this code must run, in plain words. The user reads it next to the code before allowing it.",
                });
                let required = tool["inputSchema"]["required"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !required.iter().any(|r| r == "explanation") {
                    let mut required = required;
                    required.push("explanation".into());
                    tool["inputSchema"]["required"] = Value::Array(required);
                }
                let description = tool["description"].as_str().unwrap_or("").to_owned();
                tool["description"] = format!(
                    "{description} Needs the user's approval: they see your code and `explanation`."
                )
                .into();
            }
        }
        if policy.trust != BrowserTrust::ReadOnly {
            tools.push(approval_tool_definition());
        }
        Ok(tools)
    }

    fn call_tool(&self, route: &str, name: &str, args: Value) -> Value {
        let (policy, page_url) = {
            let state = self.lock_state();
            (state.policy.clone(), state.page.clone())
        };
        match policy.evaluate(name, &args, &page_url) {
            Verdict::Deny(reason) => return tool_text(reason, true),
            Verdict::Ask(request) => {
                if let Err(reason) = self.ask(route, request) {
                    return tool_text(reason, true);
                }
                // The wait can be long: the user may have lowered the level meanwhile.
                let now = self.lock_state().policy.clone();
                if let Verdict::Deny(reason) = now.evaluate(name, &args, &page_url) {
                    return tool_text(reason, true);
                }
                if name == APPROVAL_TOOL {
                    return tool_text("Approved by the user. You may do this action now.", false);
                }
            }
            Verdict::Allow => {}
        }
        let mut forwarded = args;
        if CODE_TOOLS.contains(&name) {
            if let Some(map) = forwarded.as_object_mut() {
                map.remove("explanation");
            }
        }
        // Opening a page makes it this session's page at once, so the next write is checked
        // against it even when the result does not repeat the address.
        if let Some(target) = opened_address(name, &forwarded) {
            self.lock_state().page = target;
        }
        match self.upstream_rpc("tools/call", json!({"name": name, "arguments": forwarded})) {
            Ok(result) => {
                if let Some(url) = page_url_of(&result) {
                    self.lock_state().page = url;
                }
                result
            }
            Err(error) => tool_text(format!("The browser is not available: {error}"), true),
        }
    }

    /// Shows the card and waits. Everything but an explicit Allow is an error text for the agent.
    fn ask(&self, route: &str, request: ApprovalRequest) -> Result<(), String> {
        let sender = self
            .approvers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(route)
            .map(|a| a.sender.clone());
        let Some(sender) = sender else {
            return Err("Denied: there is no way to ask the user right now.".into());
        };
        let (reply, answer) = mpsc::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        let ask = Ask {
            request,
            reply,
            abandoned: abandoned.clone(),
        };
        if sender.send(ask).is_err() {
            return Err("Denied: there is no way to ask the user right now.".into());
        }
        match answer.recv_timeout(self.timeout) {
            Ok(true) => Ok(()),
            Ok(false) => Err(
                "Denied by the user. Do not do this action, and do not try another way to do it."
                    .into(),
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                abandoned.store(true, Ordering::SeqCst);
                Err("Denied: the user did not answer in time.".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("Denied: the request was not answered (the run ended or was stopped).".into())
            }
        }
    }

    /// One request to the Playwright server, starting a session first when there is none. A
    /// refused request (HTTP 400 or 404: a stale session) is repeated once on a new session;
    /// any other failure is not, because the call may have run.
    fn upstream_rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut last = String::new();
        for _ in 0..2 {
            let session = self.ensure_session()?;
            let message = json!({
                "jsonrpc": "2.0",
                "id": self.next_id.fetch_add(1, Ordering::SeqCst),
                "method": method,
                "params": params,
            });
            let upstream = self.lock_state().upstream.clone();
            match upstream.post(&session.key, session.id.as_deref(), &message) {
                Ok((Some(reply), _)) => {
                    return match reply.get("error") {
                        Some(error) => Err(error["message"]
                            .as_str()
                            .unwrap_or("The browser server refused the request.")
                            .to_owned()),
                        None => Ok(reply["result"].clone()),
                    }
                }
                Ok((None, _)) => return Err("The browser server sent no reply.".into()),
                Err(error) => {
                    // A server that keeps its one MCP session for its whole life (stdio) must not
                    // be asked to initialize again: it would refuse, for ever.
                    if !upstream.session_survives(&error) {
                        self.lock_state().session = None;
                    }
                    let stale = error == "HTTP 400" || error == "HTTP 404";
                    last = error;
                    if !stale {
                        break;
                    }
                }
            }
        }
        Err(last)
    }

    fn ensure_session(&self) -> Result<Session, String> {
        let _one_at_a_time = self.init.lock().unwrap_or_else(|e| e.into_inner());
        let (key, upstream) = {
            let state = self.lock_state();
            if let Some(session) = state
                .session
                .as_ref()
                .filter(|s| s.key == state.upstream_key)
            {
                return Ok(session.clone());
            }
            (state.upstream_key.clone(), state.upstream.clone())
        };
        let hello = json!({
            "jsonrpc": "2.0",
            "id": self.next_id.fetch_add(1, Ordering::SeqCst),
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "monoptah-gate", "version": env!("CARGO_PKG_VERSION")},
            },
        });
        let (reply, id) = upstream.post(&key, None, &hello)?;
        if reply.as_ref().is_none_or(|r| r.get("error").is_some()) {
            return Err("The browser server refused to start a session.".into());
        }
        upstream.post(
            &key,
            id.as_deref(),
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )?;
        let session = Session { key, id };
        self.lock_state().session = Some(session.clone());
        Ok(session)
    }
}

/// The page a tool result reports: a `- Page URL: ...` line, or for a tab list the address of
/// the tab marked `(current)`, so a tab switch or a new tab moves the remembered page too.
fn page_url_of(result: &Value) -> Option<String> {
    let lines: Vec<&str> = result["content"]
        .as_array()?
        .iter()
        .filter_map(|part| part["text"].as_str())
        .flat_map(str::lines)
        .collect();
    let direct = lines.iter().find_map(|line| {
        line.trim()
            .trim_start_matches("- ")
            .strip_prefix("Page URL:")
            .map(|url| url.trim().to_owned())
    });
    direct
        .or_else(|| {
            lines
                .iter()
                .filter(|line| line.contains("(current)"))
                .find_map(|line| {
                    let tail = line.rsplit_once("](")?.1;
                    Some(tail.split(')').next()?.trim().to_owned())
                })
        })
        .filter(|url| !url.is_empty())
}

fn approval_tool_definition() -> Value {
    json!({
        "name": APPROVAL_TOOL,
        "title": "Ask the user for approval",
        "description": "Ask the user for approval BEFORE a risky action, and wait for the answer. Call it before: a payment, purchase, subscription, transfer, or entering card or bank details (payment); sending a message, email or content to someone (send_message); posting, commenting, reviewing or publishing (post_publish); anything else irreversible or outward-facing (other). Do the action only if the result says Approved. If it says Denied, do not do the action and do not try another way.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "category": {"type": "string", "enum": Category::AGENT_CHOICES, "description": "payment, send_message, post_publish or other"},
                "action": {"type": "string", "description": "What you are about to do, in one sentence."},
                "url": {"type": "string", "description": "The page the action happens on."},
                "details": {"type": "string", "description": "Exactly what will be sent, posted or paid: the text, the recipient, the amount."},
            },
            "required": ["category", "action"],
        },
    })
}

#[cfg(test)]
mod tests;
