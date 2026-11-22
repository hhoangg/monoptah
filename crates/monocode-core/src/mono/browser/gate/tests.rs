use super::*;
use crate::mono::browser::KNOWN_TOOLS;
use std::sync::atomic::AtomicUsize;

const FINANCE: &str = "https://www.paypal.com/checkoutnow";
const SHOP: &str = "https://shop.example/cart";

fn policy(trust: BrowserTrust) -> Policy {
    Policy::new(trust, vec!["https://secure.mybank.example".into()])
}
fn ask_of(verdict: Verdict) -> ApprovalRequest {
    match verdict {
        Verdict::Ask(request) => request,
        other => panic!("expected an approval, got {other:?}"),
    }
}
fn denied(verdict: &Verdict) -> &str {
    match verdict {
        Verdict::Deny(reason) => reason,
        other => panic!("expected a denial, got {other:?}"),
    }
}

// ---- Policy ----

#[test]
fn level_1_allows_only_the_read_tools_and_denies_writes_and_code() {
    let p = policy(BrowserTrust::ReadOnly);
    for tool in READ_ONLY_TOOLS {
        let args = match tool {
            "browser_tabs" => json!({"action": "list"}),
            "browser_navigate" => json!({"url": "https://news.example"}),
            "browser_press_key" => json!({"key": "PageDown"}),
            _ => json!({}),
        };
        assert_eq!(p.evaluate(tool, &args, SHOP), Verdict::Allow, "{tool}");
    }
    for tool in KNOWN_TOOLS {
        if READ_ONLY_TOOLS.contains(&tool) {
            continue;
        }
        let verdict = p.evaluate(
            tool,
            &json!({"function": "() => 1", "explanation": "why", "code": "async()=>1"}),
            SHOP,
        );
        assert!(
            matches!(verdict, Verdict::Deny(_)),
            "{tool} must not run at Level 1: {verdict:?}"
        );
    }
    // press_key scrolls at Level 1, but only with the scroll keys: Enter, Space, Tab, a letter or
    // a chord could press a button, submit a form or type.
    for key in SCROLL_KEYS {
        assert_eq!(
            p.evaluate("browser_press_key", &json!({"key": key}), SHOP),
            Verdict::Allow,
            "{key}"
        );
    }
    for args in [
        json!({"key": "Enter"}),
        json!({"key": " "}),
        json!({"key": "Space"}),
        json!({"key": "Tab"}),
        json!({"key": "a"}),
        json!({"key": "Control+End"}),
        json!({"key": "pagedown"}),
        json!({"key": ""}),
        json!({}),
        json!({"key": ["End"]}),
    ] {
        let verdict = p.evaluate("browser_press_key", &args, SHOP);
        assert!(denied(&verdict).contains("Level 1"), "{args}");
    }
    // Tabs: list, open and switch are reads; closing one is not.
    for action in ["list", "new", "select"] {
        assert_eq!(
            p.evaluate("browser_tabs", &json!({"action": action}), SHOP),
            Verdict::Allow
        );
    }
    assert!(matches!(
        p.evaluate("browser_tabs", &json!({"action": "close"}), SHOP),
        Verdict::Deny(_)
    ));
    // Nothing to approve at Level 1, and an unknown tool never runs.
    assert!(matches!(
        p.evaluate(
            APPROVAL_TOOL,
            &json!({"category": "payment", "action": "pay"}),
            SHOP
        ),
        Verdict::Deny(_)
    ));
    assert!(matches!(
        p.evaluate("browser_new_thing", &json!({}), SHOP),
        Verdict::Deny(_)
    ));
}

#[test]
fn route_unroute_and_resize_are_denied_at_every_level() {
    for trust in BrowserTrust::ALL {
        for tool in ALWAYS_BLOCKED_TOOLS {
            let verdict = policy(trust).evaluate(tool, &json!({"width": 10}), SHOP);
            assert!(
                denied(&verdict).contains("never available"),
                "{trust:?} {tool}"
            );
        }
    }
}

#[test]
fn ordinary_page_tools_run_on_their_own_at_levels_2_and_3() {
    for trust in [BrowserTrust::AskRisky, BrowserTrust::Autonomous] {
        let p = policy(trust);
        for (tool, args) in [
            (
                "browser_click",
                json!({"target": "e3", "element": "Add to cart"}),
            ),
            (
                "browser_type",
                json!({"target": "e4", "text": "hello", "submit": false}),
            ),
            (
                "browser_fill_form",
                json!({"fields": [{"name": "Name", "value": "Ada"}]}),
            ),
            ("browser_press_key", json!({"key": "PageDown"})),
            ("browser_navigate", json!({"url": "https://news.example"})),
            ("browser_mouse_wheel", json!({"deltaX": 0, "deltaY": 600})),
            ("browser_tabs", json!({"action": "close"})),
        ] {
            assert_eq!(
                p.evaluate(tool, &args, SHOP),
                Verdict::Allow,
                "{trust:?} {tool}"
            );
        }
        // A page the gate has not seen yet is not treated as a bank.
        assert_eq!(
            p.evaluate("browser_click", &json!({"target": "e3"}), ""),
            Verdict::Allow
        );
    }
}

#[test]
fn code_runs_only_through_a_card_with_the_code_and_the_explanation_at_level_2() {
    let p = policy(BrowserTrust::AskRisky);
    let code = "() => document.title";
    let request = ask_of(p.evaluate(
        "browser_evaluate",
        &json!({"function": code, "explanation": "Read the page title"}),
        SHOP,
    ));
    assert_eq!(request.category, Category::Code);
    assert_eq!(request.code.as_deref(), Some(code));
    assert_eq!(request.explanation, "Read the page title");
    assert_eq!(request.url, SHOP);
    let detail = request.detail();
    assert!(detail.contains(code) && detail.contains("Why: Read the page title"));
    assert!(detail.contains(&format!("Page: {SHOP}")));
    assert_eq!(request.title(), "Allow the browser to run code?");

    let server_side = ask_of(p.evaluate(
        "browser_run_code_unsafe",
        &json!({"code": "async (page) => page.title()", "explanation": "Needs Playwright"}),
        SHOP,
    ));
    assert_eq!(
        server_side.code.as_deref(),
        Some("async (page) => page.title()")
    );
    assert!(server_side.action.contains("browser server"));

    // No explanation: the agent is told to give one, nothing is asked.
    for tool in CODE_TOOLS {
        let verdict = p.evaluate(
            tool,
            &json!({"function": "() => 1", "code": "async()=>1", "explanation": "  "}),
            SHOP,
        );
        assert!(denied(&verdict).contains("explanation"), "{tool}");
    }
    // Code loaded from a file is code the user never sees.
    let verdict = p.evaluate(
        "browser_run_code_unsafe",
        &json!({"filename": "x.js", "explanation": "because"}),
        SHOP,
    );
    assert!(denied(&verdict).contains("must see the code"));
    // Missing and oversized code.
    assert!(matches!(
        p.evaluate("browser_evaluate", &json!({"explanation": "why"}), SHOP),
        Verdict::Deny(_)
    ));
    let long = "x".repeat(MAX_CODE_CHARS + 1);
    assert!(denied(&p.evaluate(
        "browser_evaluate",
        &json!({"function": long, "explanation": "why"}),
        SHOP
    ))
    .contains("too long"));
    // Even harmless-looking code asks at Level 2.
    assert!(matches!(
        p.evaluate(
            "browser_evaluate",
            &json!({"function": "() => 1 + 1", "explanation": "math"}),
            SHOP
        ),
        Verdict::Ask(_)
    ));
}

#[test]
fn level_3_asks_for_every_piece_of_code_because_no_text_check_can_prove_it_safe() {
    let p = policy(BrowserTrust::Autonomous);
    for code in [
        "() => document.title",
        "() => document.querySelectorAll('a').length",
        // Bypasses a keyword list; every one of them must still reach the user.
        "() => document.querySelector('button')['cli'+'ck']()",
        "() => window['fe'+'tch']('/x', {method: 'POST'})",
        "() => Object.assign(document.body.style, {display: 'none'})",
        "() => document.body.setAttribute('x', 'y')",
        "() => document.body.insertAdjacentHTML('beforeend', '<i>')",
    ] {
        let verdict = p.evaluate(
            "browser_evaluate",
            &json!({"function": code, "explanation": "why"}),
            SHOP,
        );
        assert!(matches!(verdict, Verdict::Ask(_)), "{code}");
    }
    assert!(matches!(
        p.evaluate(
            "browser_run_code_unsafe",
            &json!({"code": "async (page) => page.title()", "explanation": "why"}),
            SHOP
        ),
        Verdict::Ask(_)
    ));
}

#[test]
fn only_http_and_https_addresses_open_at_every_level() {
    for trust in BrowserTrust::ALL {
        let p = policy(trust);
        for bad in [
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "file:///etc/passwd",
            "ftp://example.com/x",
            "chrome://settings",
            "about:blank",
            "https://",
            "not a url",
            "",
        ] {
            let verdict = p.evaluate("browser_navigate", &json!({"url": bad}), SHOP);
            assert!(
                denied(&verdict).contains("http and https"),
                "{trust:?} {bad:?}"
            );
            let tab = p.evaluate("browser_tabs", &json!({"action": "new", "url": bad}), SHOP);
            if bad.is_empty() {
                assert_eq!(tab, Verdict::Allow, "a new empty tab is fine");
            } else {
                assert!(matches!(tab, Verdict::Deny(_)), "{trust:?} {bad:?}");
            }
        }
        for good in ["https://news.example/a?b=1", "http://localhost:3000/"] {
            assert_eq!(
                p.evaluate("browser_navigate", &json!({"url": good}), SHOP),
                Verdict::Allow,
                "{good}"
            );
        }
    }
}

#[test]
fn navigating_to_a_finance_page_is_allowed_and_makes_the_next_write_ask() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::Autonomous, &upstream);
    assert_eq!(
        call(&gate, "browser_navigate", json!({"url": FINANCE}))["isError"],
        false
    );
    // The fake's result does not repeat the address of a click, but the target was remembered.
    let (handle, shown) = user(gate.attach(SHARED_ROUTE), vec![false]);
    let result = call(
        &gate,
        "browser_click",
        json!({"target": "e1", "element": "Pay"}),
    );
    handle.join().unwrap();
    assert_eq!(result["isError"], true);
    assert_eq!(shown.lock().unwrap()[0].category, Category::FinancePage);
    // A new tab opened on a finance page works the same way.
    call(&gate, "browser_navigate", json!({"url": SHOP}));
    call(
        &gate,
        "browser_tabs",
        json!({"action": "new", "url": "https://momo.vn/"}),
    );
    let (handle, shown) = user(gate.attach(SHARED_ROUTE), vec![false]);
    call(&gate, "browser_click", json!({"target": "e1"}));
    handle.join().unwrap();
    assert_eq!(shown.lock().unwrap().len(), 1);
}

#[test]
fn a_tab_list_moves_the_remembered_page_to_the_current_tab() {
    let tabs = json!({"content": [{"type": "text", "text":
        "### Open tabs\n- 0: [Shop](https://shop.example/cart)\n- 1: (current) [Pay](https://www.paypal.com/checkoutnow)"}]});
    assert_eq!(page_url_of(&tabs).as_deref(), Some(FINANCE));
    let page = json!({"content": [{"type": "text", "text": "- Page URL: https://shop.example/cart\n- Page Title: x"}]});
    assert_eq!(page_url_of(&page).as_deref(), Some(SHOP));
    assert_eq!(
        page_url_of(&json!({"content": [{"type": "text", "text": "ok"}]})),
        None
    );
}

#[test]
fn page_changes_on_finance_and_blocked_origins_need_approval_at_levels_2_and_3() {
    for trust in [BrowserTrust::AskRisky, BrowserTrust::Autonomous] {
        let p = policy(trust);
        for url in [
            FINANCE,
            "https://secure.mybank.example/accounts",
            "https://momo.vn/",
            "https://shop.example/checkout/step-2",
            "https://pay.google.com/gp/w/u/0/home",
            "https://secure.chase.com/web/auth",
        ] {
            let request = ask_of(p.evaluate(
                "browser_click",
                &json!({"target": "e9", "element": "Confirm"}),
                url,
            ));
            assert_eq!(request.category, Category::FinancePage, "{url}");
            assert_eq!(request.url, url);
            assert!(request.action.contains("Confirm"));
        }
        // Reading there is not a change.
        assert_eq!(
            p.evaluate("browser_snapshot", &json!({}), FINANCE),
            Verdict::Allow
        );
        assert!(!is_finance_url("https://news.example/article", &[]));
        assert!(!is_finance_url("not a url", &[]));
    }
}

#[test]
fn a_typed_card_number_needs_approval() {
    let p = policy(BrowserTrust::Autonomous);
    for args in [
        json!({"target": "e1", "text": "4111 1111 1111 1111"}),
        json!({"fields": [{"name": "Card", "value": "4111-1111-1111-1111"}]}),
        json!({"values": ["5500005555555559"]}),
    ] {
        let request = ask_of(p.evaluate("browser_type", &args, SHOP));
        assert_eq!(request.category, Category::CardData);
    }
    // Digits that are not a card number do not ask: wrong checksum, or too short.
    for args in [
        json!({"target": "e1", "text": "4111 1111 1111 1112"}),
        json!({"target": "e1", "text": "2026-11-22"}),
        json!({"target": "e1", "text": "call me on 0912345678"}),
    ] {
        assert_eq!(p.evaluate("browser_type", &args, SHOP), Verdict::Allow);
    }
    assert!(contains_card_number(
        &json!({"a": ["x", {"b": "378282246310005"}]})
    ));
}

#[test]
fn the_approval_tool_builds_a_card_with_category_action_url_and_details() {
    let p = policy(BrowserTrust::AskRisky);
    let request = ask_of(p.evaluate(
        APPROVAL_TOOL,
        &json!({
            "category": "send_message",
            "action": "Send the reply to Lan",
            "url": "https://mail.example/compose",
            "details": "Text: See you at 5",
        }),
        SHOP,
    ));
    assert_eq!(request.category, Category::SendMessage);
    assert_eq!(request.title(), "Allow the browser to send this?");
    let detail = request.detail();
    for needle in [
        "Sending a message",
        "Action: Send the reply to Lan",
        "Page: https://mail.example/compose",
        "Details: Text: See you at 5",
    ] {
        assert!(detail.contains(needle), "{needle} in {detail}");
    }
    assert!(request.code.is_none());
    // The page the gate saw is the default URL.
    let request = ask_of(p.evaluate(
        APPROVAL_TOOL,
        &json!({"category": "payment", "action": "Pay 20 USD"}),
        SHOP,
    ));
    assert_eq!(request.url, SHOP);
    assert_eq!(request.category, Category::Payment);
    // Bad input is refused with a hint.
    assert!(denied(&p.evaluate(APPROVAL_TOOL, &json!({"action": "x"}), SHOP)).contains("category"));
    assert!(denied(&p.evaluate(
        APPROVAL_TOOL,
        &json!({"category": "payment", "action": " "}),
        SHOP
    ))
    .contains("action"));
    assert!(matches!(
        p.evaluate(
            APPROVAL_TOOL,
            &json!({"category": "evaluate", "action": "x"}),
            SHOP
        ),
        Verdict::Deny(_)
    ));
    // Level 3 offers the same tool (money, and anything the agent is unsure about).
    assert!(matches!(
        policy(BrowserTrust::Autonomous).evaluate(
            APPROVAL_TOOL,
            &json!({"category": "payment", "action": "Pay"}),
            SHOP
        ),
        Verdict::Ask(_)
    ));
}

// ---- The gate in front of a fake Playwright server ----

struct FakeUpstream {
    posts: Mutex<Vec<Value>>,
    sessions: Mutex<Vec<Option<String>>>,
    page: Mutex<String>,
    fail_with: Mutex<Option<String>>,
    /// Refuses the next `tools/call` once, as a server does that lost the session.
    refuse_next_call: Mutex<Option<String>>,
    /// Like a stdio server: the gate keeps its session after an error.
    keep_session: AtomicBool,
    initializes: AtomicUsize,
}
impl FakeUpstream {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            posts: Mutex::default(),
            sessions: Mutex::default(),
            page: Mutex::new(SHOP.into()),
            fail_with: Mutex::new(None),
            refuse_next_call: Mutex::new(None),
            keep_session: AtomicBool::new(false),
            initializes: AtomicUsize::new(0),
        })
    }
    fn calls(&self, method: &str) -> Vec<Value> {
        self.posts
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m["method"] == method)
            .cloned()
            .collect()
    }
    fn tool_calls(&self) -> Vec<(String, Value)> {
        self.calls("tools/call")
            .into_iter()
            .map(|m| {
                (
                    m["params"]["name"].as_str().unwrap().to_owned(),
                    m["params"]["arguments"].clone(),
                )
            })
            .collect()
    }
}
impl Upstream for FakeUpstream {
    fn session_survives(&self, _: &str) -> bool {
        self.keep_session.load(Ordering::SeqCst)
    }
    fn post(
        &self,
        _: &str,
        session: Option<&str>,
        message: &Value,
    ) -> Result<(Option<Value>, Option<String>), String> {
        self.posts.lock().unwrap().push(message.clone());
        self.sessions
            .lock()
            .unwrap()
            .push(session.map(str::to_owned));
        if let Some(error) = self.fail_with.lock().unwrap().clone() {
            return Err(error);
        }
        if message["method"] == "tools/call" {
            if let Some(error) = self.refuse_next_call.lock().unwrap().take() {
                return Err(error);
            }
        }
        let id = message["id"].clone();
        let ok = |result: Value| {
            Ok((
                Some(json!({"jsonrpc": "2.0", "id": id, "result": result})),
                None,
            ))
        };
        match message["method"].as_str().unwrap_or("") {
            "initialize" => {
                self.initializes.fetch_add(1, Ordering::SeqCst);
                Ok((
                    Some(json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": {}}})),
                    Some("sess-1".into()),
                ))
            }
            "tools/list" => ok(json!({"tools": KNOWN_TOOLS.iter().map(|name| json!({
                "name": name,
                "description": format!("{name} does a thing."),
                "inputSchema": {"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"]},
            })).collect::<Vec<_>>()})),
            "tools/call" => {
                if message["params"]["name"] == "browser_navigate" {
                    *self.page.lock().unwrap() = message["params"]["arguments"]["url"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                }
                let name = message["params"]["name"].as_str().unwrap();
                let page = if matches!(name, "browser_navigate" | "browser_snapshot") {
                    format!(
                        "\n### Page\n- Page URL: {}\n- Page Title: t",
                        self.page.lock().unwrap()
                    )
                } else {
                    String::new()
                };
                ok(
                    json!({"content": [{"type": "text", "text": format!("### Result\nran {name}{page}")}], "isError": false}),
                )
            }
            _ => Ok((None, None)),
        }
    }
}

fn gate_with(trust: BrowserTrust, upstream: &Arc<FakeUpstream>, timeout: Duration) -> Arc<Gate> {
    Gate::start_with_timeout(
        upstream.clone(),
        policy(trust),
        "http://127.0.0.1:41000/mcp",
        timeout,
    )
    .unwrap()
}
fn gate(trust: BrowserTrust, upstream: &Arc<FakeUpstream>) -> Arc<Gate> {
    gate_with(trust, upstream, Duration::from_secs(5))
}
fn rpc(gate: &Gate, method: &str, params: Value) -> Value {
    rpc_on(gate, SHARED_ROUTE, method, params)
}
fn rpc_on(gate: &Gate, route: &str, method: &str, params: Value) -> Value {
    gate.handle(
        route,
        json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}),
    )
    .expect("a reply")
}
fn call_on(gate: &Gate, route: &str, name: &str, args: Value) -> Value {
    rpc_on(
        gate,
        route,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )["result"]
        .clone()
}
fn call(gate: &Gate, name: &str, args: Value) -> Value {
    rpc(gate, "tools/call", json!({"name": name, "arguments": args}))["result"].clone()
}
fn text_of(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}
fn names(reply: &Value) -> Vec<String> {
    reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

/// A user: answers each card in turn from `answers` and records what it showed.
fn user(
    attached: Attached,
    answers: Vec<bool>,
) -> (
    std::thread::JoinHandle<()>,
    Arc<Mutex<Vec<ApprovalRequest>>>,
) {
    let shown = Arc::new(Mutex::new(Vec::new()));
    let seen = shown.clone();
    let handle = std::thread::spawn(move || {
        for answer in answers {
            let Ok(ask) = attached.receiver.recv_timeout(Duration::from_secs(5)) else {
                return;
            };
            seen.lock().unwrap().push(ask.request.clone());
            let _ = ask.reply.send(answer);
        }
    });
    (handle, shown)
}

#[test]
fn the_gate_lists_the_tools_of_the_level_and_adds_the_approval_tool_above_level_1() {
    let upstream = FakeUpstream::new();
    let level_1 = names(&rpc(
        &gate(BrowserTrust::ReadOnly, &upstream),
        "tools/list",
        json!({}),
    ));
    let mut expected: Vec<String> = READ_ONLY_TOOLS.iter().map(|t| (*t).to_owned()).collect();
    expected.sort();
    let mut got = level_1.clone();
    got.sort();
    assert_eq!(got, expected);
    assert!(!level_1.contains(&APPROVAL_TOOL.to_owned()));

    for trust in [BrowserTrust::AskRisky, BrowserTrust::Autonomous] {
        let reply = rpc(&gate(trust, &upstream), "tools/list", json!({}));
        let listed = names(&reply);
        assert!(listed.contains(&APPROVAL_TOOL.to_owned()));
        for tool in ALWAYS_BLOCKED_TOOLS {
            assert!(!listed.contains(&tool.to_owned()), "{tool}");
        }
        for tool in [
            "browser_click",
            "browser_evaluate",
            "browser_run_code_unsafe",
        ] {
            assert!(listed.contains(&tool.to_owned()), "{tool}");
        }
        // The code tools gain a required `explanation` argument; the others are untouched.
        let tools = reply["result"]["tools"].as_array().unwrap();
        let find = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap();
        for tool in CODE_TOOLS {
            let schema = &find(tool)["inputSchema"];
            assert_eq!(schema["properties"]["explanation"]["type"], "string");
            assert!(schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("explanation")));
            assert!(find(tool)["description"]
                .as_str()
                .unwrap()
                .contains("approval"));
        }
        assert!(find("browser_click")["inputSchema"]["properties"]["explanation"].is_null());
        let approval = find(APPROVAL_TOOL);
        for needle in ["payment", "send_message", "post_publish"] {
            assert!(approval["inputSchema"]["properties"]["category"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(needle)));
        }
        assert_eq!(
            approval["inputSchema"]["required"],
            json!(["category", "action"])
        );
    }
}

#[test]
fn level_1_calls_never_reach_the_server_unless_they_read() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::ReadOnly, &upstream);
    for tool in [
        "browser_click",
        "browser_press_key",
        "browser_evaluate",
        "browser_type",
    ] {
        let result = call(
            &gate,
            tool,
            json!({"target": "e1", "key": "Enter", "function": "()=>1", "explanation": "x"}),
        );
        assert_eq!(result["isError"], true, "{tool}");
    }
    assert!(
        upstream.tool_calls().is_empty(),
        "{:?}",
        upstream.tool_calls()
    );
    let result = call(&gate, "browser_snapshot", json!({}));
    assert_eq!(result["isError"], false);
    assert_eq!(upstream.tool_calls().len(), 1);
}

#[test]
fn ordinary_clicks_are_forwarded_with_one_upstream_session_and_the_page_is_remembered() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    // No approver attached at all: an ordinary action does not need one.
    let result = call(&gate, "browser_click", json!({"target": "e1"}));
    assert_eq!(result["isError"], false);
    assert!(text_of(&result).contains("ran browser_click"));
    call(
        &gate,
        "browser_navigate",
        json!({"url": "https://other.example"}),
    );
    call(&gate, "browser_snapshot", json!({}));
    assert_eq!(upstream.initializes.load(Ordering::SeqCst), 1);
    assert_eq!(upstream.calls("notifications/initialized").len(), 1);
    // Every call after the handshake carries the session the server named.
    let sessions = upstream.sessions.lock().unwrap().clone();
    assert_eq!(sessions[0], None);
    assert!(sessions[2..].iter().all(|s| s.as_deref() == Some("sess-1")));
    // The page the tools reported is what the finance check looks at.
    call(&gate, "browser_navigate", json!({"url": FINANCE}));
    let (handle, answers) = user(gate.attach(SHARED_ROUTE), vec![false]);
    let result = call(
        &gate,
        "browser_click",
        json!({"target": "e2", "element": "Pay now"}),
    );
    handle.join().unwrap();
    assert_eq!(result["isError"], true);
    assert_eq!(answers.lock().unwrap()[0].category, Category::FinancePage);
    assert!(!upstream
        .tool_calls()
        .iter()
        .any(|(n, a)| n == "browser_click" && a["target"] == "e2"));
}

#[test]
fn code_runs_only_after_allow_and_without_the_explanation_argument() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let (handle, shown) = user(gate.attach(SHARED_ROUTE), vec![true, false]);
    let args = json!({"function": "() => document.title", "explanation": "Read the title"});

    let allowed = call(&gate, "browser_evaluate", args.clone());
    assert_eq!(allowed["isError"], false);
    let forwarded = upstream.tool_calls();
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded[0].0, "browser_evaluate");
    assert_eq!(forwarded[0].1, json!({"function": "() => document.title"}));

    let refused = call(
        &gate,
        "browser_run_code_unsafe",
        json!({"code": "async (page) => 1", "explanation": "Needs Playwright"}),
    );
    handle.join().unwrap();
    assert_eq!(refused["isError"], true);
    assert!(text_of(&refused).contains("Denied by the user"));
    assert_eq!(
        upstream.tool_calls().len(),
        1,
        "the denied call never reached the server"
    );

    let shown = shown.lock().unwrap();
    assert_eq!(shown[0].code.as_deref(), Some("() => document.title"));
    assert_eq!(shown[0].explanation, "Read the title");
    assert_eq!(shown[1].code.as_deref(), Some("async (page) => 1"));
}

#[test]
fn level_3_code_goes_through_a_card_too() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::Autonomous, &upstream);
    let args = json!({"function": "() => document.title", "explanation": "title"});
    let result = call(&gate, "browser_evaluate", args.clone());
    assert_eq!(result["isError"], true, "no way to ask: denied");
    let (handle, shown) = user(gate.attach(SHARED_ROUTE), vec![true]);
    assert_eq!(call(&gate, "browser_evaluate", args)["isError"], false);
    handle.join().unwrap();
    assert_eq!(shown.lock().unwrap()[0].category, Category::Code);
}

#[test]
fn two_sessions_of_one_mono_keep_their_cards_apart_and_share_the_page_check() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let args = json!({"category": "payment", "action": "Pay"});
    let a = gate.attach("s1");
    let b = gate.attach("s2");
    // A's card shows in A only.
    let a_user = {
        let shown = Arc::new(Mutex::new(Vec::new()));
        let seen = shown.clone();
        let handle = std::thread::spawn(move || {
            let ask = a.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            seen.lock().unwrap().push(ask.request.clone());
            ask.reply.send(true).unwrap();
            a // keep it attached until the test has finished
        });
        (handle, shown)
    };
    assert_eq!(
        call_on(&gate, "s1", APPROVAL_TOOL, args.clone())["isError"],
        false
    );
    assert_eq!(a_user.1.lock().unwrap().len(), 1);
    assert!(b.try_recv().is_err(), "B must not see A's card");
    let a = a_user.0.join().unwrap();
    // B ends: A still gets its cards, nothing of A is denied.
    drop(b);
    let handle = std::thread::spawn(move || {
        let ask = a.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        ask.reply.send(true).unwrap();
    });
    assert_eq!(
        call_on(&gate, "s1", APPROVAL_TOOL, args.clone())["isError"],
        false
    );
    handle.join().unwrap();
    // A session that is not attached is denied, whoever else is.
    assert_eq!(call_on(&gate, "s2", APPROVAL_TOOL, args)["isError"], true);

    // The browser and its current tab are shared: a bank tab one session opened is checked for
    // the other one too.
    call_on(&gate, "s1", "browser_navigate", json!({"url": FINANCE}));
    assert_eq!(
        call_on(&gate, "s2", "browser_snapshot", json!({}))["isError"],
        false
    );
    let (handle, shown) = user(gate.attach("s2"), vec![false]);
    assert_eq!(
        call_on(&gate, "s2", "browser_click", json!({"target": "e1"}))["isError"],
        true
    );
    handle.join().unwrap();
    assert_eq!(shown.lock().unwrap()[0].category, Category::FinancePage);
}

#[test]
fn a_level_dropped_while_the_card_was_open_is_honored_after_allow() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let attached = gate.attach(SHARED_ROUTE);
    let lower = gate.clone();
    let handle = std::thread::spawn(move || {
        let ask = attached
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        lower.set_policy(policy(BrowserTrust::ReadOnly));
        ask.reply.send(true).unwrap();
    });
    let result = call(
        &gate,
        "browser_evaluate",
        json!({"function": "() => 1", "explanation": "why"}),
    );
    handle.join().unwrap();
    assert_eq!(result["isError"], true);
    assert!(text_of(&result).contains("Level 1"));
    assert!(
        upstream.tool_calls().is_empty(),
        "the allowed call was not forwarded"
    );
}

#[test]
fn it_fails_closed_without_a_channel_on_timeout_and_when_nobody_answers() {
    let upstream = FakeUpstream::new();
    let args = json!({"function": "() => 1", "explanation": "why"});

    // No run attached.
    let gate_a = gate(BrowserTrust::AskRisky, &upstream);
    let result = call(&gate_a, "browser_evaluate", args.clone());
    assert_eq!(result["isError"], true);
    assert!(text_of(&result).contains("no way to ask"));

    // The run detached (its turn ended): same.
    drop(gate_a.attach(SHARED_ROUTE));
    assert_eq!(
        call(&gate_a, "browser_evaluate", args.clone())["isError"],
        true
    );

    // Attached, but the user never answers: denied after the timeout, and the card is marked
    // abandoned so the run can withdraw it.
    let gate_b = gate_with(
        BrowserTrust::AskRisky,
        &upstream,
        Duration::from_millis(100),
    );
    let attached = gate_b.attach(SHARED_ROUTE);
    let result = call(&gate_b, "browser_evaluate", args.clone());
    assert!(text_of(&result).contains("did not answer in time"));
    assert_eq!(result["isError"], true);
    let late = attached.receiver.try_recv().unwrap();
    assert!(late.abandoned.load(Ordering::SeqCst));

    // The run drops the request without answering (stopped): denied, not hung.
    let gate_c = gate(BrowserTrust::AskRisky, &upstream);
    let attached = gate_c.attach(SHARED_ROUTE);
    let dropper = std::thread::spawn(move || {
        let ask = attached
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        drop(ask);
    });
    let result = call(&gate_c, "browser_evaluate", args);
    dropper.join().unwrap();
    assert_eq!(result["isError"], true);
    assert!(text_of(&result).contains("not answered"));

    assert!(
        upstream.tool_calls().is_empty(),
        "nothing reached the server"
    );
}

#[test]
fn the_approval_tool_reports_the_decision_to_the_agent() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let (handle, shown) = user(gate.attach(SHARED_ROUTE), vec![true, false]);
    let args = json!({
        "category": "post_publish",
        "action": "Publish the comment",
        "url": "https://blog.example/p/1",
        "details": "Text: Great post",
    });
    let allowed = call(&gate, APPROVAL_TOOL, args.clone());
    assert_eq!(allowed["isError"], false);
    assert!(text_of(&allowed).starts_with("Approved"));
    let denied = call(&gate, APPROVAL_TOOL, args);
    handle.join().unwrap();
    assert_eq!(denied["isError"], true);
    assert!(text_of(&denied).contains("Denied by the user"));
    let shown = shown.lock().unwrap();
    assert_eq!(shown[0].category, Category::PostPublish);
    assert_eq!(shown[0].url, "https://blog.example/p/1");
    assert!(shown[0].detail().contains("Text: Great post"));
    // The approval tool itself is never forwarded to the Playwright server.
    assert!(upstream.tool_calls().is_empty());
}

#[test]
fn an_unreachable_server_is_reported_and_a_refused_stale_session_is_retried_once() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    *upstream.fail_with.lock().unwrap() = Some("connection refused".into());
    let result = call(&gate, "browser_snapshot", json!({}));
    assert_eq!(result["isError"], true);
    assert!(text_of(&result).contains("not available"));
    *upstream.fail_with.lock().unwrap() = None;
    assert_eq!(call(&gate, "browser_snapshot", json!({}))["isError"], false);
    assert_eq!(upstream.tool_calls().len(), 1);

    // A refused request (the server forgot the session) did not run: repeated on a new session.
    *upstream.refuse_next_call.lock().unwrap() = Some("HTTP 404".into());
    assert_eq!(call(&gate, "browser_snapshot", json!({}))["isError"], false);
    assert_eq!(upstream.initializes.load(Ordering::SeqCst), 2);
    assert_eq!(upstream.tool_calls().len(), 3, "refused, then repeated");
    // Any other failure may have run the call, so it is not repeated.
    *upstream.refuse_next_call.lock().unwrap() = Some("timed out".into());
    assert_eq!(
        call(&gate, "browser_click", json!({"target": "e1"}))["isError"],
        true
    );
    assert_eq!(upstream.tool_calls().len(), 4);
}

#[test]
fn configure_applies_a_new_level_and_a_new_server_address() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    assert_eq!(
        call(&gate, "browser_click", json!({"target": "e1"}))["isError"],
        false
    );
    gate.configure(
        policy(BrowserTrust::ReadOnly),
        "http://127.0.0.1:41000/mcp",
        upstream.clone(),
    );
    assert_eq!(
        call(&gate, "browser_click", json!({"target": "e1"}))["isError"],
        true
    );
    assert_eq!(upstream.initializes.load(Ordering::SeqCst), 1);
    // The server restarted on another port: a new session on the new address.
    gate.configure(
        policy(BrowserTrust::AskRisky),
        "http://127.0.0.1:41001/mcp",
        upstream.clone(),
    );
    assert_eq!(call(&gate, "browser_snapshot", json!({}))["isError"], false);
    assert_eq!(upstream.initializes.load(Ordering::SeqCst), 2);
}

#[test]
fn a_newer_run_takes_over_the_cards_and_an_older_one_cannot_detach_it() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let old = gate.attach(SHARED_ROUTE);
    let new = gate.attach(SHARED_ROUTE);
    drop(old);
    let (handle, shown) = user(new, vec![true]);
    let result = call(
        &gate,
        APPROVAL_TOOL,
        json!({"category": "payment", "action": "Pay"}),
    );
    handle.join().unwrap();
    assert_eq!(result["isError"], false);
    assert_eq!(shown.lock().unwrap().len(), 1);
}

// ---- The loopback server ----

#[allow(clippy::result_large_err)]
fn post(url: &str, body: &Value) -> Result<ureq::Response, ureq::Error> {
    ureq::post(url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json, text/event-stream")
        .send_string(&body.to_string())
}

#[test]
fn the_loopback_server_speaks_mcp_over_http_and_hides_behind_its_secret() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let url = gate.url();
    assert!(url.starts_with("http://127.0.0.1:") && url.ends_with("/mcp"));

    let init = post(&url, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})).unwrap();
    assert_eq!(init.status(), 200);
    assert!(init
        .header("content-type")
        .unwrap()
        .contains("application/json"));
    let init: Value = serde_json::from_str(&init.into_string().unwrap()).unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    let note = post(
        &url,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .unwrap();
    assert_eq!(note.status(), 202);

    let list: Value = serde_json::from_str(
        &post(
            &url,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .unwrap()
        .into_string()
        .unwrap(),
    )
    .unwrap();
    assert!(names(&list).contains(&APPROVAL_TOOL.to_owned()));

    let called: Value = serde_json::from_str(
        &post(&url, &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "browser_click", "arguments": {"target": "e1"}}}))
            .unwrap()
            .into_string()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(called["result"]["isError"], false);

    // A batch is answered as a batch.
    let batch: Value = serde_json::from_str(
        &post(&url, &json!([{"jsonrpc": "2.0", "id": 4, "method": "ping"}, {"jsonrpc": "2.0", "method": "notifications/initialized"}]))
            .unwrap()
            .into_string()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(batch.as_array().unwrap().len(), 1);

    // No stream and no session end: 405. Wrong secret: 404. Not JSON: 400.
    assert!(matches!(
        ureq::get(&url).call(),
        Err(ureq::Error::Status(405, _))
    ));
    let wrong = url.replace(url.rsplit('/').nth(2).unwrap(), "0000");
    assert!(matches!(
        post(&wrong, &json!({})),
        Err(ureq::Error::Status(404, _))
    ));
    assert!(matches!(
        ureq::post(&url).send_string("not json"),
        Err(ureq::Error::Status(400, _))
    ));
}

#[test]
fn a_request_for_another_host_name_is_refused_and_a_chunked_body_is_read() {
    use std::io::{Read, Write};
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let url = gate.url();
    let (authority, path) = url.trim_start_matches("http://").split_once('/').unwrap();
    let send = |host: &str, chunked: bool| {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut stream = std::net::TcpStream::connect(authority).unwrap();
        let request = if chunked {
            format!(
                "POST /{path} HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
                body.len()
            )
        } else {
            format!(
                "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
        };
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    };
    assert!(send("evil.example", false).starts_with("HTTP/1.1 403"));
    assert!(send(authority, false).starts_with("HTTP/1.1 200"));
    let chunked = send(authority, true);
    assert!(chunked.starts_with("HTTP/1.1 200") && chunked.contains("\"result\":{}"));
}

#[test]
fn dropping_the_gate_stops_its_server() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let authority = gate
        .url()
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    drop(gate);
    for _ in 0..50 {
        if std::net::TcpStream::connect(&authority).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the gate still listens");
}

#[test]
fn a_gate_is_kept_per_mono_and_follows_its_settings() {
    use crate::mono::browser::server::{tests_support::FakeBackend, BrowserServers};
    let upstream = FakeUpstream::new();
    let servers = BrowserServers::new(Arc::new(FakeBackend::default()), Duration::from_secs(60))
        .with_upstream(upstream.clone());
    let first = servers
        .gate_for(
            "m1",
            "http://127.0.0.1:41000/mcp",
            policy(BrowserTrust::AskRisky),
        )
        .unwrap();
    let again = servers
        .gate_for(
            "m1",
            "http://127.0.0.1:41000/mcp",
            policy(BrowserTrust::ReadOnly),
        )
        .unwrap();
    assert_eq!(
        first.url(),
        again.url(),
        "a parked agent keeps a working URL"
    );
    assert_eq!(
        call(&again, "browser_click", json!({"target": "e1"}))["isError"],
        true
    );
    let other = servers
        .gate_for(
            "m2",
            "http://127.0.0.1:41000/mcp",
            policy(BrowserTrust::AskRisky),
        )
        .unwrap();
    assert_ne!(first.url(), other.url());
    servers.stop("m1");
    let fresh = servers
        .gate_for(
            "m1",
            "http://127.0.0.1:41000/mcp",
            policy(BrowserTrust::AskRisky),
        )
        .unwrap();
    assert_ne!(first.url(), fresh.url());
    servers.stop_everything();
}

#[test]
fn a_full_server_answers_503_instead_of_dropping_the_connection() {
    use std::io::Read;
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let url = gate.url();
    let (authority, _) = url.trim_start_matches("http://").split_once('/').unwrap();
    // Idle connections that never finish their request fill every slot.
    let held: Vec<_> = (0..32)
        .map(|_| std::net::TcpStream::connect(authority).unwrap())
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    // The refusal comes without the client having to say anything.
    let mut stream = std::net::TcpStream::connect(authority).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 503"), "{reply}");
    drop(held);
}

#[cfg(unix)]
#[test]
fn a_stdio_server_is_reached_over_its_pipes_and_its_end_is_reported() {
    use std::process::{Command, Stdio};
    // Answers request 100 with a result and stops after request 101.
    let script = r#"while IFS= read -r line; do case "$line" in *'"id":100'*) echo '{"jsonrpc":"2.0","method":"notifications/message"}'; echo '{"jsonrpc":"2.0","id":100,"result":{"ok":1}}';; *'"id":101'*) exit 0;; esac; done"#;
    let mut child = Command::new("sh")
        .args(["-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let upstream = StdioUpstream::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let (reply, session) = upstream
        .post(
            "stdio:1",
            None,
            &json!({"jsonrpc": "2.0", "id": 100, "method": "x"}),
        )
        .unwrap();
    assert_eq!(reply.unwrap()["result"], json!({"ok": 1}));
    assert_eq!(session, None);
    // A notification has no reply.
    let (none, _) = upstream
        .post(
            "stdio:1",
            None,
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .unwrap();
    assert!(none.is_none());
    // The server exits: the waiting call fails and the next one says the server stopped.
    assert!(upstream
        .post(
            "stdio:1",
            None,
            &json!({"jsonrpc": "2.0", "id": 101, "method": "x"})
        )
        .is_err());
    child.wait().unwrap();
    for _ in 0..50 {
        if !upstream.is_open() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!upstream.is_open());
    assert_eq!(
        upstream
            .post(
                "stdio:1",
                None,
                &json!({"jsonrpc": "2.0", "id": 102, "method": "x"})
            )
            .unwrap_err(),
        "The browser server stopped."
    );
}

#[test]
fn a_gate_keeps_its_session_after_an_error_only_when_the_server_has_just_one() {
    for (keep, initializes) in [(false, 2), (true, 1)] {
        let upstream = FakeUpstream::new();
        upstream.keep_session.store(keep, Ordering::SeqCst);
        let gate = gate(BrowserTrust::AskRisky, &upstream);
        assert_eq!(call(&gate, "browser_snapshot", json!({}))["isError"], false);
        *upstream.refuse_next_call.lock().unwrap() = Some("timed out".into());
        assert_eq!(call(&gate, "browser_snapshot", json!({}))["isError"], true);
        assert_eq!(call(&gate, "browser_snapshot", json!({}))["isError"], false);
        assert_eq!(
            upstream.initializes.load(Ordering::SeqCst),
            initializes,
            "{keep}"
        );
    }
}

#[test]
fn routes_are_random_per_session_stable_and_only_issued_ones_answer() {
    let upstream = FakeUpstream::new();
    let gate = gate(BrowserTrust::AskRisky, &upstream);
    let (a, b, none) = (
        gate.route_for(Some(1)),
        gate.route_for(Some(2)),
        gate.route_for(None),
    );
    assert_eq!(a, gate.route_for(Some(1)));
    assert!(a != b && a != none && b != none);
    assert!(a.len() >= 32 && a.bytes().all(|c| c.is_ascii_hexdigit()));
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    assert_eq!(post(&gate.url_for(&a), &ping).unwrap().status(), 200);
    assert_eq!(post(&gate.url(), &ping).unwrap().status(), 200);
    // A guessed or made-up route is not found, even with the right secret.
    for guess in ["s1", "shared", "0"] {
        assert!(matches!(
            post(&gate.url_for(guess), &ping),
            Err(ureq::Error::Status(404, _))
        ));
    }
}

#[cfg(unix)]
#[test]
fn a_stdio_server_that_stops_answering_is_given_up_and_never_asked_to_start_again() {
    use std::process::{Command, Stdio};
    // Reads everything, answers nothing.
    let mut child = Command::new("sh")
        .args(["-c", "while IFS= read -r line; do :; done"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let upstream = StdioUpstream::over_with_timeout(
        Box::new(child.stdin.take().unwrap()),
        Box::new(child.stdout.take().unwrap()),
        Duration::from_millis(200),
    );
    assert!(upstream.session_survives("anything"));
    let error = upstream
        .post(
            "stdio:1",
            None,
            &json!({"jsonrpc": "2.0", "id": 100, "method": "x"}),
        )
        .unwrap_err();
    assert!(error.contains("in time"));
    // Given up: no further call is sent, and the child sees its stdin end and exits.
    assert!(!upstream.is_open());
    assert_eq!(
        upstream
            .post(
                "stdio:1",
                None,
                &json!({"jsonrpc": "2.0", "id": 101, "method": "x"})
            )
            .unwrap_err(),
        "The browser server stopped."
    );
    child.wait().unwrap();
}
