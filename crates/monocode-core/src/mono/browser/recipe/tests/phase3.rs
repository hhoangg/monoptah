//! Phase 3 tests: recipes that change things (type, fill, select, press, click a button, send),
//! their risk classes, parameters, approvals per trust level, the run time checks and the repair
//! rules. Same scripted [`Web`] as the other recipe tests: no real Chrome.
use super::*;
use crate::mono::browser::gate::approval_required;
use crate::mono::browser::recipe::risk;

const CHAT: &str = "https://news.example/chat";
const CHAT_BODY: &str = r#"- main [ref=e20]:
  - heading "Team chat" [level=1] [ref=e21]
  - textbox "Message" [ref=e30]
  - button "Save draft" [ref=e32]
  - button "Send" [ref=e31]
  - button "Buy now" [ref=e33]
  - textbox "Password" [ref=e34]
  - combobox "Channel" [ref=e35]
"#;
const MSG_BOX: &str = "internal:role=textbox[name=\"Message\"s]";
const SEND_BTN: &str = "internal:role=button[name=\"Send\"s]";
const SAVE_BTN: &str = "internal:role=button[name=\"Save draft\"s]";
const BUY_BTN: &str = "internal:role=button[name=\"Buy now\"s]";
const PASS_BOX: &str = "internal:role=textbox[name=\"Password\"s]";
const CHANNEL_BOX: &str = "internal:role=combobox[name=\"Channel\"s]";
const TYPED: &str = "hello team";

fn chat_page() -> PageDef {
    let mut page = PageDef {
        title: "Chat".into(),
        body: CHAT_BODY.into(),
        ..PageDef::default()
    };
    for (selector, probe) in [
        (MSG_BOX, "- textbox \"Message\" [ref=e30]"),
        (SEND_BTN, "- button \"Send\" [ref=e31]"),
        (SAVE_BTN, "- button \"Save draft\" [ref=e32]"),
        (BUY_BTN, "- button \"Buy now\" [ref=e33]"),
        (PASS_BOX, "- textbox \"Password\" [ref=e34]"),
        (CHANNEL_BOX, "- combobox \"Channel\" [ref=e35]"),
    ] {
        page.selectors
            .insert(selector.into(), (probe.into(), probe.into()));
    }
    for target in [SEND_BTN, BUY_BTN, "e31", "e33"] {
        page.clicks.insert(target.into(), ARCHIVE.into());
    }
    page
}
fn chat_web() -> Arc<Web> {
    let web = Web::new();
    web.pages.lock().unwrap().insert(CHAT.into(), chat_page());
    web
}
fn chat_rig(trust: BrowserTrust) -> Rig {
    Rig::with(trust, chat_web())
}
fn target(role: &str, name: &str) -> Target {
    Target {
        candidates: vec![locator(role, name)],
    }
}
fn extract_step() -> Step {
    sample_recipe().steps[1].clone()
}
fn chat_open() -> Step {
    Step::Open {
        url: CHAT.into(),
        expect: vec![Expect::UrlMatches("^https://news\\.example/chat".into())],
    }
}
fn typed(value: &str) -> Step {
    Step::Type {
        target: target("textbox", "Message"),
        value: value.into(),
        expect: vec![Expect::RolePresent {
            role: "textbox".into(),
            name: Some("Message".into()),
        }],
    }
}
fn send_button() -> Step {
    Step::Send {
        target: target("button", "Send"),
        action: SendAction::Send,
        expect: vec![Expect::UrlMatches("^https://news\\.example/archive".into())],
    }
}
fn approve(kind: ApproveKind) -> Step {
    Step::Approve { category: kind }
}
fn text_param() -> Param {
    Param {
        name: "text".into(),
        kind: ParamKind::String,
        description: "what to say".into(),
        default: None,
    }
}
fn n_declared() -> Param {
    Param {
        name: "n".into(),
        kind: ParamKind::Integer,
        description: "how many".into(),
        default: Some(json!(2)),
    }
}
/// Open the chat, type a message, send it, read the page that follows.
fn chat_recipe() -> Recipe {
    let mut recipe = sample_recipe();
    recipe.id = "rcp_0c0ffee1".into();
    recipe.intent = "chat.say".into();
    recipe.params = vec![text_param(), n_declared()];
    recipe.steps = vec![
        chat_open(),
        approve(ApproveKind::Other),
        typed("{text}"),
        approve(ApproveKind::SendMessage),
        send_button(),
        extract_step(),
    ];
    recipe.risk = Risk::Send;
    recipe.stats.verified = true;
    recipe
}
fn say() -> Value {
    json!({"text": TYPED, "n": 2})
}
fn typed_calls(rig: &Rig) -> Vec<Value> {
    rig.web
        .tool_calls()
        .into_iter()
        .filter(|c| c.0 == "browser_type")
        .map(|c| c.1)
        .collect()
}
fn acted_calls(rig: &Rig) -> usize {
    rig.web
        .names()
        .iter()
        .filter(|n| {
            matches!(
                n.as_str(),
                "browser_type" | "browser_click" | "browser_fill_form" | "browser_select_option"
            ) || n.as_str() == "browser_press_key"
        })
        .count()
}

// ---- Classification and the model ----

#[test]
fn each_step_has_a_risk_class_and_the_recipe_risk_is_computed_never_trusted() {
    let by = |step: Step| step.risk();
    assert_eq!(by(chat_open()), Risk::Read);
    assert_eq!(by(extract_step()), Risk::Read);
    assert_eq!(by(approve(ApproveKind::Other)), Risk::Read);
    assert_eq!(by(typed("{text}")), Risk::Write);
    let act = |name: &str| Step::Act {
        target: target("button", name),
        expect: vec![Expect::TextVisible("x".into())],
    };
    assert_eq!(by(act("Save draft")), Risk::Write);
    assert_eq!(by(act("Delete conversation")), Risk::Write);
    assert_eq!(by(act("Buy now")), Risk::Money);
    assert_eq!(by(act("Place order")), Risk::Money);
    assert_eq!(by(act("Post comment")), Risk::Send);
    assert_eq!(by(act("Send")), Risk::Send);
    assert_eq!(by(send_button()), Risk::Send);
    // A send button that names a payment is a payment.
    let pay = Step::Send {
        target: target("button", "Pay and send"),
        action: SendAction::Send,
        expect: vec![Expect::TextVisible("x".into())],
    };
    assert_eq!(by(pay), Risk::Money);
    let press = |key: &str| Step::Press {
        key: key.into(),
        expect: vec![Expect::TextVisible("x".into())],
    };
    assert_eq!(by(press("Enter")), Risk::Send);
    assert_eq!(by(press("Escape")), Risk::Write);
    assert_eq!(by(act("Post comment")).as_str(), "send");
    // The recipe risk is the highest step, and a file that says less is invalid.
    let recipe = chat_recipe();
    assert_eq!(recipe.computed_risk(), Risk::Send);
    assert_eq!(recipe.min_level(), 2);
    assert!(recipe.validate().is_ok());
    let mut lying = recipe.clone();
    lying.risk = Risk::Read;
    assert!(lying.validate().unwrap_err().contains("computed"));
    let mut high = recipe.clone();
    high.risk = Risk::Money;
    assert!(high.validate().is_err());
    // A read recipe is still level 1 and `read`.
    assert_eq!(sample_recipe().computed_risk(), Risk::Read);
    assert_eq!(sample_recipe().min_level(), 1);
    // The live label class is what a name says; a text box is never a send.
    assert_eq!(risk::label_class("button", "Send").risk, Risk::Send);
    assert_eq!(
        risk::label_class("button", "Reply").category,
        Category::PostPublish
    );
    assert_eq!(
        risk::label_class("textbox", "Send a message").risk,
        Risk::Write
    );
    assert_eq!(risk::label_class("button", "OK").risk, Risk::Write);
}

#[test]
fn approval_asks_follow_the_gate_rule_for_each_level() {
    let asks = [
        Category::Payment,
        Category::SendMessage,
        Category::PostPublish,
        Category::Other,
        Category::Code,
        Category::FinancePage,
        Category::CardData,
    ];
    for category in asks {
        assert!(!approval_required(BrowserTrust::ReadOnly, category));
        assert!(
            approval_required(BrowserTrust::AskRisky, category),
            "{category:?}"
        );
    }
    for category in [
        Category::Payment,
        Category::Code,
        Category::FinancePage,
        Category::CardData,
    ] {
        assert!(
            approval_required(BrowserTrust::Autonomous, category),
            "{category:?}"
        );
    }
    for category in [
        Category::SendMessage,
        Category::PostPublish,
        Category::Other,
    ] {
        assert!(
            !approval_required(BrowserTrust::Autonomous, category),
            "{category:?}"
        );
    }
    // Saving a recipe has its own card and is not an approval of an action.
    assert!(!approval_required(BrowserTrust::AskRisky, Category::Recipe));
}

#[test]
fn what_is_typed_is_always_one_parameter_and_the_target_never_holds_one() {
    let ok = chat_recipe();
    assert!(ok.validate().is_ok(), "{:?}", ok.validate());
    let with = |edit: &dyn Fn(&mut Recipe)| {
        let mut r = ok.clone();
        edit(&mut r);
        r
    };
    let cases = [
        (
            "a typed literal",
            with(&|r| r.steps[2] = typed("hello team")),
        ),
        (
            "text around a parameter",
            with(&|r| r.steps[2] = typed("hi {text}")),
        ),
        ("two parameters", with(&|r| r.steps[2] = typed("{text}{n}"))),
        (
            "a parameter in the target of a write",
            with(&|r| {
                r.steps[2] = Step::Type {
                    target: target("textbox", "Message {text}"),
                    value: "{text}".into(),
                    expect: vec![Expect::TextVisible("x".into())],
                }
            }),
        ),
        (
            "a parameter in the send button",
            with(&|r| {
                r.steps[4] = Step::Send {
                    target: target("button", "{text}"),
                    action: SendAction::Send,
                    expect: vec![Expect::TextVisible("x".into())],
                }
            }),
        ),
        (
            "typing into a password field",
            with(&|r| {
                r.steps[2] = Step::Type {
                    target: target("textbox", "Password"),
                    value: "{text}".into(),
                    expect: vec![Expect::TextVisible("x".into())],
                }
            }),
        ),
        (
            "typing into a one-time code field",
            with(&|r| {
                r.steps[2] = Step::Type {
                    target: target("textbox", "Verification code"),
                    value: "{text}".into(),
                    expect: vec![Expect::TextVisible("x".into())],
                }
            }),
        ),
        (
            "a card number field",
            with(&|r| {
                r.steps[2] = Step::Type {
                    target: target("textbox", "Card number"),
                    value: "{text}".into(),
                    expect: vec![Expect::TextVisible("x".into())],
                }
            }),
        ),
        (
            "a write with no assertion",
            with(&|r| {
                r.steps[2] = Step::Type {
                    target: target("textbox", "Message"),
                    value: "{text}".into(),
                    expect: vec![],
                }
            }),
        ),
        (
            "a press with a scroll key",
            with(&|r| {
                r.steps.insert(
                    3,
                    Step::Press {
                        key: "PageDown".into(),
                        expect: vec![Expect::TextVisible("x".into())],
                    },
                )
            }),
        ),
        (
            "a text-only locator for a button",
            with(&|r| {
                r.steps[4] = Step::Send {
                    target: Target {
                        candidates: vec![Candidate {
                            text: Some("Send".into()),
                            ..Candidate::default()
                        }],
                    },
                    action: SendAction::Send,
                    expect: vec![Expect::TextVisible("x".into())],
                }
            }),
        ),
        (
            "a checkbox that is not true or false",
            with(&|r| {
                r.steps[2] = Step::Fill {
                    fields: vec![FormField {
                        target: target("checkbox", "Notify"),
                        kind: FieldKind::Checkbox,
                        value: "yes".into(),
                    }],
                    expect: vec![Expect::TextVisible("x".into())],
                }
            }),
        ),
    ];
    for (what, recipe) in cases {
        assert!(recipe.validate().is_err(), "{what} must be refused");
    }
    // A parameter named like a credential, or with a credential as its default.
    for (name, default) in [
        ("passcode", json!("hello")),
        ("note", json!("4111 1111 1111 1111")),
        (
            "note",
            json!("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghij"),
        ),
        ("note", json!("sk-abcdefghijklmnopqrstuvwx")),
        ("note", json!("123456")),
    ] {
        let mut r = ok.clone();
        r.params[0].name = name.into();
        r.params[0].default = Some(default.clone());
        r.steps[2] = typed(&format!("{{{name}}}"));
        assert!(r.validate().is_err(), "{name}={default} must be refused");
    }
    // A typed parameter has no default at all, even an ordinary one (the caller gives it).
    let mut with_default = ok.clone();
    with_default.params[0].default = Some(json!("Thanks!"));
    assert!(with_default.validate().unwrap_err().contains("no default"));
}

#[test]
fn every_acting_step_needs_an_approve_step_of_a_strong_enough_kind() {
    let ok = chat_recipe();
    let mut no_approve = ok.clone();
    no_approve.steps.remove(1);
    assert!(no_approve.validate().unwrap_err().contains("approve"));
    // A send guarded only by the approval for plain writes.
    let mut weak = ok.clone();
    weak.steps[3] = approve(ApproveKind::Other);
    assert!(weak.validate().is_err());
    // An approve that guards nothing.
    let mut loose = ok.clone();
    loose.steps.insert(1, approve(ApproveKind::PostPublish));
    assert!(loose.validate().is_err());
    // A page change between the approval and the write drops the cover.
    let mut moved = ok.clone();
    moved.steps.insert(2, chat_open());
    assert!(moved.validate().is_err());
    // A payment approval covers a send (the strictest kind).
    let mut strong = ok.clone();
    strong.steps[3] = approve(ApproveKind::Payment);
    assert!(strong.validate().is_ok());
    // One approval for plain writes covers a run of them, not the send after.
    let mut run = ok.clone();
    run.steps.insert(3, typed("{text}"));
    assert!(run.validate().is_ok());
    let governed = governing(&run.steps);
    assert_eq!(governed[2], Some(1));
    assert_eq!(governed[3], Some(1));
    assert_eq!(governed[5], Some(4));
}

#[test]
fn bound_values_are_never_credentials_and_a_value_cannot_expand_into_a_step() {
    let params = [text_param()];
    for secret in [
        "4111 1111 1111 1111",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghij",
        "sk-abcdefghijklmnopqrstuvwx",
        "ghp_abcdefghijklmnopqrstuvwxyz0123",
        "-----BEGIN PRIVATE KEY----- abc",
        "Bearer abcdefgh12345",
        "AKIAABCDEFGHIJKLMNOP",
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMN0123",
    ] {
        let given = Map::from_iter([("text".to_owned(), json!(secret))]);
        assert!(bind_params(&params, &given).is_err(), "{secret}");
    }
    // A value is data: `{n}` inside it is not filled in a second time.
    let values = BTreeMap::from([
        ("text".to_owned(), "{n}".to_owned()),
        ("n".into(), "9".into()),
    ]);
    assert_eq!(substitute("{text}", &values, Encode::Raw), "{n}");
    assert!(risk::otp_like("123456") && !risk::otp_like("12"));
    assert_eq!(
        risk::scrub("fill('hello team')", &["hello team".into()]),
        "fill('[value hidden]')"
    );
}

// ---- Running ----

#[test]
fn a_write_recipe_runs_at_level_2_and_the_cards_show_the_step_the_page_and_the_values() {
    let rig = chat_rig(BrowserTrust::AskRisky);
    approved_recipe(&rig, &chat_recipe());
    let (handle, shown) = user(rig.attach(), vec![true, true]);
    let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
    handle.join().unwrap();
    assert!(ok, "{result}");
    assert_eq!(result["data"].as_array().unwrap().len(), 2);
    let cards = shown.lock().unwrap().clone();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].category, Category::Other);
    assert_eq!(cards[1].category, Category::SendMessage);
    let first = cards[0].detail();
    for needle in [
        "Type \"hello team\" into the textbox \"Message\"",
        "Step 3",
        CHAT,
        "chat.say",
    ] {
        assert!(first.contains(needle), "{needle} in {first}");
    }
    let second = cards[1].detail();
    assert!(
        second.contains("SEND by clicking the button \"Send\""),
        "{second}"
    );
    // The text reached the page only after the first card, without `submit`.
    let names = rig.web.names();
    let typed_at = names.iter().position(|n| n == "browser_type").unwrap();
    let click_at = names.iter().position(|n| n == "browser_click").unwrap();
    assert!(typed_at < click_at);
    let calls = typed_calls(&rig);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["text"], TYPED);
    assert!(calls[0].get("submit").is_none());
    assert_eq!(rig.store().get("rcp_0c0ffee1").unwrap().stats.ok, 1);
}

#[test]
fn a_no_on_the_card_stops_the_run_before_anything_is_typed() {
    let rig = chat_rig(BrowserTrust::AskRisky);
    approved_recipe(&rig, &chat_recipe());
    let (handle, _) = user(rig.attach(), vec![false]);
    let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
    handle.join().unwrap();
    assert!(!ok);
    assert_eq!(acted_calls(&rig), 0, "{:?}", rig.web.names());
    assert!(result["reason"].as_str().unwrap().contains("Denied"));
    // The second card: the text is typed, the send is refused and not repeated by the Mono.
    let rig = chat_rig(BrowserTrust::AskRisky);
    approved_recipe(&rig, &chat_recipe());
    let (handle, _) = user(rig.attach(), vec![true, false]);
    let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
    handle.join().unwrap();
    assert!(!ok);
    assert_eq!(typed_calls(&rig).len(), 1);
    assert!(!rig.web.names().iter().any(|n| n == "browser_click"));
    assert_eq!(result["changed_things"], json!([3]));
    assert!(result["next"]
        .as_str()
        .unwrap()
        .contains("never send or post twice"));
    // Nobody to ask: the same fail-closed refusal, nothing typed.
    let rig = chat_rig(BrowserTrust::AskRisky);
    approved_recipe(&rig, &chat_recipe());
    let (ok, _) = run_json(&rig, "rcp_0c0ffee1", say());
    assert!(!ok);
    assert_eq!(acted_calls(&rig), 0);
}

#[test]
fn level_1_cannot_run_a_write_recipe_and_nothing_reaches_the_browser() {
    let rig = chat_rig(BrowserTrust::ReadOnly);
    approved_recipe(&rig, &chat_recipe());
    let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
    assert!(!ok);
    assert_eq!(result["op"], "level");
    assert!(result["reason"]
        .as_str()
        .unwrap()
        .contains("changes things"));
    assert!(rig.web.tool_calls().is_empty());
    // The gate itself denies each tool at Level 1, whatever a recipe says.
    let policy = Policy::new(BrowserTrust::ReadOnly, vec![]);
    for (tool, args) in [
        ("browser_type", json!({"target": "e1", "text": "x"})),
        ("browser_click", json!({"target": "e1"})),
        ("browser_fill_form", json!({"fields": []})),
        (
            "browser_select_option",
            json!({"target": "e1", "values": ["a"]}),
        ),
        ("browser_press_key", json!({"key": "Enter"})),
        (
            "browser_request_approval",
            json!({"category": "other", "action": "x"}),
        ),
    ] {
        assert!(
            matches!(policy.evaluate(tool, &args, CHAT), Verdict::Deny(_)),
            "{tool}"
        );
    }
}

#[test]
fn level_3_drops_the_approvals_it_does_not_ask_for_and_keeps_money() {
    let rig = chat_rig(BrowserTrust::Autonomous);
    approved_recipe(&rig, &chat_recipe());
    // No card for the write or the send: nobody is attached, and the run still passes.
    let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
    assert!(ok, "{result}");
    assert_eq!(typed_calls(&rig).len(), 1);
    assert!(result["notes"].to_string().contains("Level 3 does not ask"));
    // A payment asks at Level 3 too.
    let mut buy = chat_recipe();
    buy.id = "rcp_0c0ffee2".into();
    buy.intent = "chat.buy".into();
    buy.steps = vec![
        chat_open(),
        approve(ApproveKind::Payment),
        Step::Act {
            target: target("button", "Buy now"),
            expect: vec![Expect::UrlMatches("^https://news\\.example/archive".into())],
        },
        extract_step(),
    ];
    buy.params = vec![n_declared()];
    buy.risk = Risk::Money;
    approved_recipe(&rig, &buy);
    let clicks = |rig: &Rig| {
        rig.web
            .names()
            .iter()
            .filter(|n| *n == "browser_click")
            .count()
    };
    let before = clicks(&rig);
    let (ok, _) = run_json(&rig, "rcp_0c0ffee2", json!({}));
    assert!(!ok, "no one to ask: the payment is denied");
    assert_eq!(clicks(&rig), before);
    let rig = chat_rig(BrowserTrust::Autonomous);
    approved_recipe(&rig, &buy);
    let (handle, shown) = user(rig.attach(), vec![true]);
    let (ok, _) = run_json(&rig, "rcp_0c0ffee2", json!({}));
    handle.join().unwrap();
    assert!(ok);
    assert_eq!(shown.lock().unwrap()[0].category, Category::Payment);
}

#[test]
fn the_level_at_run_time_applies_not_the_level_at_record_time() {
    // Recorded at Level 2, run at Level 3: no cards. Run at Level 1: refused.
    let rig = chat_rig(BrowserTrust::AskRisky);
    approved_recipe(&rig, &chat_recipe());
    rig.gate
        .set_policy(Policy::new(BrowserTrust::Autonomous, vec![]));
    assert!(run_json(&rig, "rcp_0c0ffee1", say()).0);
    rig.gate
        .set_policy(Policy::new(BrowserTrust::ReadOnly, vec![]));
    let before = acted_calls(&rig);
    assert!(!run_json(&rig, "rcp_0c0ffee1", say()).0);
    assert_eq!(acted_calls(&rig), before);
    // The level drops in the middle of a run: the next step that changes something is refused.
    struct Shifting {
        inner: ScriptHost,
        level: Mutex<BrowserTrust>,
    }
    impl Host for Shifting {
        fn call(&self, tool: &str, args: Value) -> Result<ToolOutput, String> {
            let out = self.inner.call(tool, args);
            if tool == "browser_navigate" {
                *self.level.lock().unwrap() = BrowserTrust::ReadOnly;
            }
            out
        }
        fn page_url(&self) -> String {
            self.inner.page_url()
        }
        fn trust(&self) -> BrowserTrust {
            *self.level.lock().unwrap()
        }
        fn blocked_origins(&self) -> Vec<String> {
            vec![]
        }
        fn pause(&self, _: Duration) {}
        fn audit(&self, _: &str, _: &str, _: &str) {}
    }
    let web = chat_web();
    let host = Shifting {
        inner: ScriptHost::new(web.clone(), BrowserTrust::AskRisky),
        level: Mutex::new(BrowserTrust::AskRisky),
    };
    let outcome = runner::run(
        &host,
        &chat_recipe(),
        json!(say()).as_object().unwrap(),
        fast(),
    );
    let failure = outcome.failure.unwrap();
    assert!(failure.reason.contains("Level 1"), "{}", failure.reason);
    assert!(!web.names().iter().any(|n| n == "browser_type"));
}

#[test]
fn the_risk_is_worked_out_again_from_the_live_page() {
    // Recorded as a plain write ("Save draft"), but the page now says "Send now".
    let mut recipe = chat_recipe();
    recipe.steps = vec![
        chat_open(),
        approve(ApproveKind::Other),
        Step::Act {
            target: target("button", "Save draft"),
            expect: vec![Expect::UrlMatches("^https://news\\.example/archive".into())],
        },
        extract_step(),
    ];
    recipe.params = vec![n_declared()];
    recipe.risk = Risk::Write;
    assert!(recipe.validate().is_ok());
    let relabel = |rig: &Rig, label: &str| {
        let mut pages = rig.web.pages.lock().unwrap();
        let page = pages.get_mut(CHAT).unwrap();
        let probe = format!("- button \"{label}\" [ref=e32]");
        page.selectors
            .insert(SAVE_BTN.into(), (probe.clone(), probe));
        page.clicks.insert(SAVE_BTN.into(), ARCHIVE.into());
    };
    let rig = chat_rig(BrowserTrust::AskRisky);
    relabel(&rig, "Send now");
    approved_recipe(&rig, &recipe);
    // The card for the writes is answered, the second one is for the live send.
    let (handle, shown) = user(rig.attach(), vec![true, false]);
    let (ok, _) = run_json(&rig, &recipe.id, json!({}));
    handle.join().unwrap();
    assert!(!ok);
    let cards = shown.lock().unwrap().clone();
    assert_eq!(cards.len(), 2, "{cards:?}");
    assert_eq!(cards[1].category, Category::SendMessage);
    assert!(
        cards[1].detail().contains("the button \"Send now\""),
        "{}",
        cards[1].detail()
    );
    assert!(!rig.web.names().iter().any(|n| n == "browser_click"));
    // At Level 3 a send does not ask, so the same page runs.
    let rig = chat_rig(BrowserTrust::Autonomous);
    relabel(&rig, "Send now");
    approved_recipe(&rig, &recipe);
    assert!(run_json(&rig, &recipe.id, json!({})).0);
    // A payment on the live page asks at Level 3 as well.
    let rig = chat_rig(BrowserTrust::Autonomous);
    relabel(&rig, "Pay now");
    approved_recipe(&rig, &recipe);
    let (handle, shown) = user(rig.attach(), vec![false]);
    let (ok, _) = run_json(&rig, &recipe.id, json!({}));
    handle.join().unwrap();
    assert!(!ok);
    assert_eq!(shown.lock().unwrap()[0].category, Category::Payment);
    assert!(!rig.web.names().iter().any(|n| n == "browser_click"));
}

#[test]
fn a_field_that_turns_into_a_password_box_or_another_kind_of_element_is_not_typed_into() {
    for (name, role) in [("Password", "textbox"), ("Message", "button")] {
        let rig = chat_rig(BrowserTrust::Autonomous);
        rig.web
            .pages
            .lock()
            .unwrap()
            .get_mut(CHAT)
            .unwrap()
            .selectors
            .insert(
                MSG_BOX.into(),
                (
                    format!("- {role} \"{name}\" [ref=e30]"),
                    format!("- {role} \"{name}\" [ref=e30]"),
                ),
            );
        approved_recipe(&rig, &chat_recipe());
        let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
        assert!(!ok, "{name}/{role}");
        assert_eq!(typed_calls(&rig).len(), 0, "{name}/{role}");
        assert_eq!(result["op"], "type");
    }
}

#[test]
fn a_card_number_or_a_token_is_never_typed_and_a_finance_site_never_has_a_recipe() {
    let rig = chat_rig(BrowserTrust::Autonomous);
    approved_recipe(&rig, &chat_recipe());
    for secret in ["4111 1111 1111 1111", "sk-abcdefghijklmnopqrstuvwx"] {
        let (ok, result) = run_json(&rig, "rcp_0c0ffee1", json!({"text": secret, "n": 2}));
        assert!(!ok);
        assert_eq!(result["op"], "params");
    }
    assert!(rig.web.tool_calls().is_empty());
    let mut bank = chat_recipe();
    bank.origin = "https://www.paypal.com".into();
    assert!(bank.validate().is_err());
    // The gate still asks for a change on a bank page, whatever runs it.
    let policy = Policy::new(BrowserTrust::Autonomous, vec![]);
    let verdict = policy.evaluate(
        "browser_type",
        &json!({"target": "e1", "text": "x"}),
        "https://www.paypal.com/x",
    );
    assert_eq!(ask_of(verdict).category, Category::FinancePage);
    // A blocked origin stops the run before any call, as for a read recipe.
    rig.gate
        .set_policy(Policy::new(BrowserTrust::Autonomous, vec![SITE.into()]));
    let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
    assert!(!ok);
    assert_eq!(result["op"], "origin");
}

fn ask_of(verdict: Verdict) -> ApprovalRequest {
    match verdict {
        Verdict::Ask(request) => request,
        other => panic!("expected an approval, got {other:?}"),
    }
}

#[test]
fn a_changed_recipe_a_new_version_or_a_lower_risk_loses_its_approval() {
    let rig = chat_rig(BrowserTrust::Autonomous);
    let recipe = chat_recipe();
    approved_recipe(&rig, &recipe);
    assert!(run_json(&rig, &recipe.id, say()).0);
    // Another approve kind, another version, another description: each is another hash.
    type Edit = Box<dyn Fn(&mut Recipe)>;
    let edits: Vec<Edit> = vec![
        Box::new(|r| r.steps[3] = approve(ApproveKind::Payment)),
        Box::new(|r| r.version = 2),
        Box::new(|r| r.description = "Say it twice".into()),
        Box::new(|r| {
            r.steps[4] = Step::Send {
                target: target("button", "Send"),
                action: SendAction::Send,
                expect: vec![Expect::UrlMatches("^https://news\\.example/arch".into())],
            }
        }),
    ];
    for edit in edits {
        let mut edited = recipe.clone();
        edit(&mut edited);
        assert_ne!(edited.content_hash(), recipe.content_hash());
        rig.store().save(&edited).unwrap();
        let result = rig.call("recipe_run", json!({"id": recipe.id, "params": say()}));
        assert_eq!(result["isError"], true);
        assert!(text(&result).contains("not approved"));
        // Put the approved content back for the next case.
        rig.store().save(&recipe).unwrap();
    }
    // A file whose risk says `read` is invalid, so it is neither listed nor run.
    let mut lying = recipe.clone();
    lying.risk = Risk::Read;
    let path = rig.store().path_of(&recipe);
    std::fs::write(&path, serde_json::to_vec_pretty(&lying).unwrap()).unwrap();
    assert!(rig.store().get(&recipe.id).is_none());
    assert_eq!(
        rig.call("recipe_run", json!({"id": recipe.id, "params": say()}))["isError"],
        true
    );
}

#[test]
fn the_first_run_of_a_recipe_that_changes_things_asks_once_more_with_the_values() {
    let mut recipe = chat_recipe();
    recipe.stats.verified = false;
    let rig = chat_rig(BrowserTrust::Autonomous);
    approved_recipe(&rig, &recipe);
    let (handle, shown) = user(rig.attach(), vec![false]);
    let (ok, _) = run_json(&rig, &recipe.id, say());
    handle.join().unwrap();
    assert!(!ok);
    assert!(rig.web.tool_calls().is_empty());
    let detail = shown.lock().unwrap()[0].detail();
    assert!(
        detail.contains("hello team") && detail.contains("first time"),
        "{detail}"
    );
    let (handle, _) = user(rig.attach(), vec![true]);
    assert!(run_json(&rig, &recipe.id, say()).0);
    handle.join().unwrap();
    assert!(rig.store().get(&recipe.id).unwrap().stats.verified);
    // The next run needs no extra card (Level 3: nobody attached).
    assert!(run_json(&rig, &recipe.id, say()).0);
}

// ---- Typed values never leave the run ----

#[test]
fn a_typed_value_is_scrubbed_from_failures_audit_rows_and_the_trace() {
    struct Echo(ScriptHost);
    impl Host for Echo {
        fn call(&self, tool: &str, args: Value) -> Result<ToolOutput, String> {
            if tool == "browser_type" {
                // Playwright echoes what it ran, value included.
                return Ok(ToolOutput {
                    text: format!(
                        "Error: Ran Playwright code: fill('{}') failed",
                        args["text"].as_str().unwrap()
                    ),
                    is_error: true,
                });
            }
            self.0.call(tool, args)
        }
        fn page_url(&self) -> String {
            self.0.page_url()
        }
        fn trust(&self) -> BrowserTrust {
            BrowserTrust::Autonomous
        }
        fn blocked_origins(&self) -> Vec<String> {
            vec![]
        }
        fn pause(&self, _: Duration) {}
        fn audit(&self, tool: &str, url: &str, summary: &str) {
            assert!(
                !summary.contains(TYPED) && !url.contains(TYPED),
                "{tool} {summary}"
            );
            if tool == "type" {
                assert!(summary.contains("values masked"), "{summary}");
            }
        }
    }
    let host = Echo(ScriptHost::new(chat_web(), BrowserTrust::Autonomous));
    let outcome = runner::run(
        &host,
        &chat_recipe(),
        json!(say()).as_object().unwrap(),
        fast(),
    );
    let failure = outcome.failure.clone().unwrap();
    assert!(!failure.reason.contains(TYPED), "{}", failure.reason);
    assert!(failure.reason.contains("[value hidden]"));
    assert!(failure.acted && !failure.repairable);
    assert!(!outcome.result(&chat_recipe()).to_string().contains(TYPED));
    // The trace keeps the value in memory for the compile pass and never prints it.
    let mut trace = Trace::default();
    trace.note(
        "browser_type",
        &json!({"target": "e1", "text": TYPED, "submit": true}),
        "",
        "",
        true,
    );
    trace.note(
        "browser_fill_form",
        &json!({"fields": [{"name": "m", "type": "textbox", "target": "e1", "value": TYPED}]}),
        "",
        "",
        true,
    );
    trace.note(
        "browser_select_option",
        &json!({"target": "e1", "values": [TYPED]}),
        "",
        "",
        true,
    );
    assert!(trace.entries.iter().all(|e| e.args.is_null()));
    assert!(!format!("{:?}", trace.entries).contains(TYPED));
    assert_eq!(trace.entries[0].typed.text.as_deref(), Some(TYPED));
    trace.clear();
    assert!(trace.entries.is_empty());
}

#[test]
fn the_audit_log_of_a_write_run_holds_no_value_and_no_query() {
    let rig = chat_rig(BrowserTrust::Autonomous);
    approved_recipe(&rig, &chat_recipe());
    assert!(run_json(&rig, "rcp_0c0ffee1", say()).0);
    let rows = rig.approvals().recent(MONO, 100);
    assert!(rows
        .iter()
        .any(|e| e.tool == "type" && e.summary.contains("values masked")));
    for row in &rows {
        assert!(
            !row.summary.contains(TYPED) && !row.url.contains(TYPED),
            "{row:?}"
        );
    }
    let file = std::fs::read_to_string(rig.store().path_of(&chat_recipe())).unwrap();
    assert!(!file.contains(TYPED));
}

#[test]
fn a_failure_after_the_action_is_never_repaired_by_running_it_again() {
    // The page does not go where the send step expects: the click already happened.
    let rig = chat_rig(BrowserTrust::Autonomous);
    rig.web
        .pages
        .lock()
        .unwrap()
        .get_mut(CHAT)
        .unwrap()
        .clicks
        .insert(SEND_BTN.into(), HOME.into());
    approved_recipe(&rig, &chat_recipe());
    let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
    assert!(!ok);
    assert!(result.get("repair").is_none(), "{result}");
    assert_eq!(result["changed_things"], json!([3, 5]));
    assert!(result["next"]
        .as_str()
        .unwrap()
        .contains("never send or post twice"));
    // A locator that did not resolve (nothing was done yet) can still be repaired.
    let rig = chat_rig(BrowserTrust::Autonomous);
    rig.web
        .pages
        .lock()
        .unwrap()
        .get_mut(CHAT)
        .unwrap()
        .selectors
        .remove(MSG_BOX);
    approved_recipe(&rig, &chat_recipe());
    let (ok, result) = run_json(&rig, "rcp_0c0ffee1", say());
    assert!(!ok);
    assert!(result["repair"]["token"].is_string(), "{result}");
    assert!(result.get("changed_things").is_none());
}

// ---- Repair ----

fn locator_patch(role: &str, name: &str) -> Patch {
    link_patch(role, name)
}

#[test]
fn a_repair_of_a_step_that_changes_things_always_needs_the_users_card() {
    let base = chat_recipe();
    // Every kind of patch on a write, a send and a step next to an approval.
    for index in [2usize, 4] {
        for patch in [
            patch(json!({"op": "add_wait", "time_ms": 100})).unwrap(),
            patch(json!({"op": "change_assertion", "expect": [{"text_visible": "x"}]})).unwrap(),
        ] {
            let class = repair::classify(&base, index, &patch);
            assert!(
                matches!(class, Class::NeedsApproval(_)),
                "{index} {patch:?}"
            );
        }
    }
    let same = locator_patch("textbox", "message");
    let applied = repair::apply(&base, 2, &same, 2).unwrap();
    assert!(matches!(applied.class, Class::NeedsApproval(_)));
    // A read step next to an approve step is not a silent repair either.
    let mut next_to = base.clone();
    next_to.steps.insert(
        1,
        Step::Click {
            target: target("link", "Archive"),
            expect: vec![Expect::UrlMatches("x".into())],
        },
    );
    let class = repair::classify(&next_to, 1, &locator_patch("link", "archive"));
    assert!(matches!(class, Class::NeedsApproval(_)), "{class:?}");
    // A read step far from any approval still keeps its approval.
    let read = sample_recipe();
    let class = repair::classify(
        &read,
        1,
        &patch(json!({"op": "add_wait", "time_ms": 10})).unwrap(),
    );
    assert_eq!(class, Class::SameClass);
}

#[test]
fn a_repair_cannot_retarget_a_send_button_or_turn_a_read_into_a_write() {
    let base = chat_recipe();
    let send = |name: &str| repair::apply(&base, 4, &locator_patch("button", name), 2);
    assert!(send("send").is_ok(), "case alone is the same words");
    // Another action, an added word, a dropped word that changes the class, another role.
    for bad in [
        "Delete",
        "Send and archive",
        "Send message",
        "Post",
        "Buy now",
        "Save draft",
    ] {
        assert!(send(bad).is_err(), "{bad}");
    }
    assert!(repair::apply(&base, 4, &locator_patch("link", "Send"), 2).is_err());
    assert!(repair::apply(&base, 4, &locator_patch("menuitem", "Send"), 2).is_err());
    // A word dropped from a two-word send is a different step.
    let mut two = base.clone();
    two.steps[4] = Step::Send {
        target: target("button", "Send and archive"),
        action: SendAction::Send,
        expect: vec![Expect::UrlMatches("x".into())],
    };
    assert!(repair::apply(&two, 4, &locator_patch("button", "Send"), 2).is_err());
    assert!(repair::apply(&two, 4, &locator_patch("button", "Archive"), 2).is_err());
    // A read link can not become a button (role), and an inserted click can not be a button.
    let click = click_recipe();
    assert!(repair::apply(&click, 1, &locator_patch("button", "Archive"), 2).is_err());
    let button_step = patch(json!({"op": "insert_step", "step": {"op": "act", "target": {"candidates": [{"role": "button", "name": "Send"}]}, "expect": [{"text_visible": "x"}]}})).unwrap();
    assert!(repair::apply(&click, 1, &button_step, 2).is_err());
    let type_step = patch(json!({"op": "insert_step", "step": {"op": "type", "target": {"candidates": [{"role": "textbox", "name": "Message"}]}, "value": "{n}", "expect": [{"text_visible": "x"}]}})).unwrap();
    assert!(repair::apply(&click, 1, &type_step, 2).is_err());
    // A form with several fields has no patch.
    let mut form = base.clone();
    form.steps[2] = Step::Fill {
        fields: vec![FormField {
            target: target("textbox", "Message"),
            kind: FieldKind::Textbox,
            value: "{text}".into(),
        }],
        expect: vec![Expect::TextVisible("x".into())],
    };
    assert!(repair::apply(&form, 2, &locator_patch("textbox", "Message"), 2).is_err());
    // A click inserted into a block of writes cuts it from its approval, so the patched recipe
    // is not valid and does not run.
    let mut block = base.clone();
    block.steps.insert(3, typed("{text}"));
    let inserted = patch(json!({"op": "insert_step", "step": {"op": "click", "target": {"candidates": [{"role": "link", "name": "Archive"}]}, "expect": [{"url_matches": "x"}]}})).unwrap();
    assert!(repair::apply(&block, 3, &inserted, 2).is_err());
    // The patch can not carry a parameter into the target of a write.
    let param = patch(
        json!({"op": "replace_candidates", "candidates": [{"role": "button", "name": "{text}"}]}),
    )
    .unwrap();
    assert!(repair::apply(&base, 4, &param, 2).is_err());
}

#[test]
fn a_locator_failure_of_a_write_step_is_repaired_through_the_users_card_and_the_re_run_asks_again()
{
    let rig = chat_rig(BrowserTrust::AskRisky);
    let mut recipe = chat_recipe();
    recipe.steps[2] = Step::Type {
        target: target("textbox", "Message"),
        value: "{text}".into(),
        expect: vec![Expect::RolePresent {
            role: "textbox".into(),
            name: None,
        }],
    };
    {
        let mut pages = rig.web.pages.lock().unwrap();
        let page = pages.get_mut(CHAT).unwrap();
        page.selectors.remove(MSG_BOX);
        let probe = "- textbox \"message\" [ref=e30]".to_owned();
        page.selectors.insert(
            "internal:role=textbox[name=\"message\"s]".into(),
            (probe.clone(), probe.clone()),
        );
        page.selectors
            .insert("internal:role=textbox".into(), (probe.clone(), probe));
    }
    approved_recipe(&rig, &recipe);
    // First run: the first card (writes) is answered yes, then the locator fails.
    let (handle, _) = user(rig.attach(), vec![true]);
    let (ok, failed) = run_json(&rig, &recipe.id, say());
    handle.join().unwrap();
    assert!(!ok);
    let token = token_of(&failed);
    assert!(typed_calls(&rig).is_empty());
    // The repair card, then the re-run asks for the writes again (the approval of the first run
    // is not in this run) and for the send.
    let patch =
        json!({"op": "replace_candidates", "candidates": [{"role": "textbox", "name": "message"}]});
    let (handle, shown) = user(rig.attach(), vec![true, true, true]);
    let (ok, fixed) = repair_with(&rig, &token, patch);
    handle.join().unwrap();
    assert!(ok, "{fixed}");
    let cards = shown.lock().unwrap().clone();
    assert_eq!(cards.len(), 3, "{cards:?}");
    assert_eq!(cards[0].category, Category::Recipe);
    assert!(
        cards[0].details.contains("changes things"),
        "{}",
        cards[0].details
    );
    assert_eq!(cards[1].category, Category::Other);
    assert_eq!(cards[2].category, Category::SendMessage);
    assert_eq!(fixed["repaired"]["approved_by_user"], true);
    assert_eq!(typed_calls(&rig).len(), 1);
    assert_eq!(rig.store().get(&recipe.id).unwrap().version, 2);
}

// ---- Recording ----

fn chat_draft(params: Value) -> Value {
    let mut draft = draft(params);
    draft["intent"] = json!("chat.say");
    draft
}
fn say_params() -> Value {
    json!([
        {"name": "text", "type": "string", "description": "what to say", "example": TYPED},
        {"name": "n", "type": "integer", "default": 2, "example": "2"},
    ])
}
fn mono_chats(rig: &Rig) {
    assert_eq!(
        rig.call("browser_navigate", json!({"url": CHAT}))["isError"],
        false
    );
    assert_eq!(rig.call("browser_snapshot", json!({}))["isError"], false);
    let typed = rig.call(
        "browser_type",
        json!({"target": "e30", "element": "Message", "text": TYPED}),
    );
    assert_eq!(typed["isError"], false);
}
fn mono_sends(rig: &Rig) {
    let click = rig.call("browser_click", json!({"target": "e31", "element": "Send"}));
    assert_eq!(click["isError"], false);
    assert_eq!(rig.call("browser_snapshot", json!({}))["isError"], false);
}

#[test]
fn a_chat_task_is_recorded_with_parameters_approve_steps_and_no_literal() {
    let rig = chat_rig(BrowserTrust::AskRisky);
    mono_chats(&rig);
    mono_sends(&rig);
    let calls_before = rig.web.names().len();
    let (handle, shown) = user(rig.attach(), vec![true]);
    let result = rig.call("recipe_record", chat_draft(say_params()));
    handle.join().unwrap();
    assert_eq!(result["isError"], false, "{}", text(&result));
    let recipe = rig.store().list().remove(0);
    let ops: Vec<&str> = recipe.steps.iter().map(Step::op).collect();
    assert_eq!(
        ops,
        ["open", "approve", "type", "approve", "send", "extract"],
        "{:?}",
        recipe.steps
    );
    assert_eq!(recipe.risk, Risk::Send);
    assert!(matches!(
        &recipe.steps[1],
        Step::Approve {
            category: ApproveKind::Other
        }
    ));
    assert!(matches!(
        &recipe.steps[3],
        Step::Approve {
            category: ApproveKind::SendMessage
        }
    ));
    assert!(matches!(&recipe.steps[2], Step::Type { value, .. } if value == "{text}"));
    assert!(
        !recipe.stats.verified,
        "a write recipe is unverified until its first run"
    );
    // The check replayed only the reading part: nothing was typed or clicked again.
    let after: Vec<String> = rig.web.names().into_iter().skip(calls_before).collect();
    assert!(
        after
            .iter()
            .all(|n| n == "browser_navigate" || n == "browser_snapshot" || n == "browser_find"),
        "{after:?}"
    );
    // The typed text is nowhere: not in the file, the card, or the audit rows.
    let file = std::fs::read_to_string(rig.store().path_of(&recipe)).unwrap();
    assert!(!file.contains(TYPED));
    let card = shown.lock().unwrap()[0].detail();
    assert!(
        !card.contains(TYPED) && card.contains("CHANGES THINGS"),
        "{card}"
    );
    assert!(
        card.contains("SEND by clicking the button \"Send\""),
        "{card}"
    );
    for row in rig.approvals().recent(MONO, 200) {
        assert!(!row.summary.contains(TYPED), "{row:?}");
    }
    // And the saved recipe runs, asking per level.
    assert!(rig.approvals().recipe_approved(MONO, &recipe));
    let (handle, shown) = user(rig.attach(), vec![true, true, true]);
    let (ok, result) = run_json(&rig, &recipe.id, say());
    handle.join().unwrap();
    assert!(ok, "{result}");
    assert_eq!(shown.lock().unwrap().len(), 3, "first run, writes, send");
}

#[test]
fn recording_refuses_what_it_cannot_parameterize_or_must_never_store() {
    let refused = |setup: &dyn Fn(&Rig), params: Value, needle: &str| {
        let rig = chat_rig(BrowserTrust::AskRisky);
        setup(&rig);
        let result = rig.call("recipe_record", chat_draft(params));
        assert_eq!(result["isError"], true, "{needle}: {}", text(&result));
        assert!(
            text(&result).contains(needle),
            "{needle} in {}",
            text(&result)
        );
        assert!(rig.store().list().is_empty());
    };
    let chat = |rig: &Rig| {
        mono_chats(rig);
        mono_sends(rig);
    };
    // The typed text is not an example of any parameter: a literal is never stored.
    refused(
        &chat,
        json!([{"name": "n", "type": "integer", "default": 2, "example": "2"}]),
        "not a parameter",
    );
    // Only a part of the typed text is an example.
    refused(
        &chat,
        json!([{"name": "text", "example": "team"}, {"name": "n", "type": "integer", "default": 2, "example": "2"}]),
        "not a parameter",
    );
    // A credential as the typed value, even when it is the example of a parameter.
    let token = "sk-abcdefghijklmnopqrstuvwx";
    refused(
        &|rig: &Rig| {
            rig.call("browser_navigate", json!({"url": CHAT}));
            rig.call("browser_snapshot", json!({}));
            rig.call(
                "browser_type",
                json!({"target": "e30", "element": "Message", "text": token}),
            );
            mono_sends(rig);
        },
        json!([{"name": "text", "example": token}, {"name": "n", "type": "integer", "default": 2, "example": "2"}]),
        "credential",
    );
    // A password field, with an ordinary-looking value.
    refused(
        &|rig: &Rig| {
            rig.call("browser_navigate", json!({"url": CHAT}));
            rig.call("browser_snapshot", json!({}));
            rig.call(
                "browser_type",
                json!({"target": "e34", "element": "Password", "text": TYPED}),
            );
            mono_sends(rig);
        },
        say_params(),
        "password",
    );
    // Code, an upload, drag and drop: the Mono did something a recipe never does.
    for (tool, args) in [
        ("browser_file_upload", json!({"paths": ["/tmp/x"]})),
        (
            "browser_drag",
            json!({"startTarget": "e1", "endTarget": "e2"}),
        ),
        ("browser_handle_dialog", json!({"accept": true})),
    ] {
        refused(
            &|rig: &Rig| {
                mono_chats(rig);
                rig.call(tool, args.clone());
                mono_sends(rig);
            },
            say_params(),
            tool,
        );
    }
    // A parameter value in the name of a button: the value would choose what is clicked.
    refused(
        &|rig: &Rig| {
            rig.web.pages.lock().unwrap().get_mut(CHAT).unwrap().body =
                CHAT_BODY.replace("button \"Send\"", "button \"Send hello team\"");
            mono_chats(rig);
            mono_sends(rig);
        },
        say_params(),
        "must not choose",
    );
    // A card number as an example.
    refused(
        &chat,
        json!([{"name": "text", "example": "4111 1111 1111 1111"}]),
        "card number",
    );
}

#[test]
fn a_mono_approval_in_the_trace_can_only_make_a_step_ask_for_a_payment_never_less() {
    let rig = chat_rig(BrowserTrust::AskRisky);
    mono_chats(&rig);
    let (handle, _) = user(rig.attach(), vec![true]);
    let asked = rig.call(
        "browser_request_approval",
        json!({"category": "payment", "action": "send it", "details": TYPED}),
    );
    handle.join().unwrap();
    assert_eq!(asked["isError"], false);
    mono_sends(&rig);
    let (handle, _) = user(rig.attach(), vec![true]);
    let result = rig.call("recipe_record", chat_draft(say_params()));
    handle.join().unwrap();
    assert_eq!(result["isError"], false, "{}", text(&result));
    let recipe = rig.store().list().remove(0);
    assert!(
        matches!(
            &recipe.steps[3],
            Step::Approve {
                category: ApproveKind::Payment
            }
        ),
        "{:?}",
        recipe.steps
    );
    // The approval text (what was to be sent) is not kept.
    let file = std::fs::read_to_string(rig.store().path_of(&recipe)).unwrap();
    assert!(!file.contains(TYPED) && !file.contains("send it"));
    // A Mono that asked for something weaker does not weaken the recipe.
    let rig = chat_rig(BrowserTrust::AskRisky);
    mono_chats(&rig);
    let (handle, _) = user(rig.attach(), vec![true]);
    rig.call(
        "browser_request_approval",
        json!({"category": "other", "action": "x"}),
    );
    handle.join().unwrap();
    mono_sends(&rig);
    let (handle, _) = user(rig.attach(), vec![true]);
    assert_eq!(
        rig.call("recipe_record", chat_draft(say_params()))["isError"],
        false
    );
    handle.join().unwrap();
    let recipe = rig.store().list().remove(0);
    assert!(matches!(
        &recipe.steps[3],
        Step::Approve {
            category: ApproveKind::SendMessage
        }
    ));
}

#[test]
fn a_button_a_form_a_menu_and_enter_are_recorded_as_their_own_steps() {
    // A payment button and a plain save button, a form fill, a menu, Enter.
    let rig = chat_rig(BrowserTrust::AskRisky);
    rig.call("browser_navigate", json!({"url": CHAT}));
    rig.call("browser_snapshot", json!({}));
    rig.call(
        "browser_select_option",
        json!({"target": "e35", "element": "Channel", "values": ["general"]}),
    );
    rig.call(
        "browser_fill_form",
        json!({"fields": [{"name": "Message", "type": "textbox", "target": "e30", "value": TYPED}]}),
    );
    rig.call(
        "browser_click",
        json!({"target": "e32", "element": "Save draft"}),
    );
    rig.call("browser_press_key", json!({"key": "Enter"}));
    rig.call(
        "browser_click",
        json!({"target": "e33", "element": "Buy now"}),
    );
    rig.call("browser_snapshot", json!({}));
    let params = json!([
        {"name": "text", "example": TYPED},
        {"name": "channel", "example": "general"},
        {"name": "n", "type": "integer", "default": 2, "example": "2"},
    ]);
    let (handle, _) = user(rig.attach(), vec![true]);
    let result = rig.call("recipe_record", chat_draft(params));
    handle.join().unwrap();
    assert_eq!(result["isError"], false, "{}", text(&result));
    let recipe = rig.store().list().remove(0);
    let ops: Vec<&str> = recipe.steps.iter().map(Step::op).collect();
    assert_eq!(
        ops,
        [
            "open", "approve", "select", "fill", "act", "approve", "press", "approve", "act",
            "extract"
        ],
        "{:?}",
        recipe.steps
    );
    assert_eq!(recipe.risk, Risk::Money);
    assert!(matches!(
        &recipe.steps[7],
        Step::Approve {
            category: ApproveKind::Payment
        }
    ));
    assert!(matches!(
        &recipe.steps[5],
        Step::Approve {
            category: ApproveKind::SendMessage
        }
    ));
    assert!(recipe.validate().is_ok());
    assert_eq!(recipe.min_level(), 2);
}

#[test]
fn a_task_that_clicks_on_a_bank_page_is_not_recorded() {
    let web = chat_web();
    let bank = "https://news.example/checkout";
    web.pages.lock().unwrap().insert(bank.into(), chat_page());
    let rig = Rig::with(BrowserTrust::AskRisky, web);
    rig.call("browser_navigate", json!({"url": bank}));
    rig.call("browser_snapshot", json!({}));
    // The page is a checkout page: the gate asks for a write there.
    let (handle, _) = user(rig.attach(), vec![true]);
    rig.call(
        "browser_type",
        json!({"target": "e30", "element": "Message", "text": TYPED}),
    );
    handle.join().unwrap();
    let result = rig.call("recipe_record", chat_draft(say_params()));
    assert_eq!(result["isError"], true);
    assert!(text(&result).contains("bank"), "{}", text(&result));
}

// ---- Findings of the security review ----

struct Capture {
    inner: ScriptHost,
    cards: Mutex<Vec<Value>>,
}
impl Host for Capture {
    fn call(&self, tool: &str, args: Value) -> Result<ToolOutput, String> {
        if tool == "browser_request_approval" {
            self.cards.lock().unwrap().push(args.clone());
        }
        self.inner.call(tool, args)
    }
    fn page_url(&self) -> String {
        self.inner.page_url()
    }
    fn trust(&self) -> BrowserTrust {
        BrowserTrust::AskRisky
    }
    fn blocked_origins(&self) -> Vec<String> {
        vec![]
    }
    fn pause(&self, _: Duration) {}
    fn audit(&self, _: &str, _: &str, _: &str) {}
}

#[test]
fn i1_a_send_card_lists_every_value_typed_on_the_page_also_after_a_repair_restart() {
    let mut recipe = chat_recipe();
    recipe.steps[4] = send_button();
    for start in [0usize, 4] {
        let web = chat_web();
        *web.current.lock().unwrap() = CHAT.into();
        let host = Capture {
            inner: ScriptHost::new(web, BrowserTrust::AskRisky),
            cards: Mutex::new(vec![]),
        };
        let options = Options {
            start_at: start,
            ..fast()
        };
        let outcome = runner::run(&host, &recipe, json!(say()).as_object().unwrap(), options);
        assert!(outcome.ok(), "{:?}", outcome.failure);
        let cards = host.cards.lock().unwrap().clone();
        let send = cards.last().unwrap();
        assert_eq!(send["category"], "send_message");
        let details = send["details"].as_str().unwrap();
        assert!(
            details.contains("sent with it") && details.contains(TYPED),
            "{start}: {details}"
        );
    }
}

#[test]
fn i2_card_numbers_are_found_through_unicode_and_odd_separators_and_split_fields() {
    let card = "4111111111111111";
    for text in [
        "4111\u{a0}1111\u{a0}1111\u{a0}1111",
        "4111\u{2009}1111\u{2009}1111\u{2009}1111",
        "4111.1111.1111.1111",
        "4111/1111/1111/1111",
        "４１１１ １１１１ １１１１ １１１１",
        "4111_1111",
    ] {
        let hit = crate::mono::browser::gate::contains_card_number(&json!(text));
        assert_eq!(hit, text != "4111_1111", "{text}");
        assert_eq!(risk::secret_value(text).is_some(), hit, "{text}");
    }
    assert!(risk::secret_value(card).is_some());
    assert!(risk::split_card(&["4111 1111", "1111 1111"]));
    assert!(!risk::split_card(&["hello", "team"]));
    for name in [
        "Card",
        "cc-number",
        "CC number",
        "Card number",
        "autocomplete cc-csc",
    ] {
        assert!(risk::secret_field(name), "{name}");
    }
    assert!(!risk::secret_field("Discard draft"));
    // Two typed halves are refused when the second one completes a card; one fill is refused whole.
    let rig = chat_rig(BrowserTrust::Autonomous);
    let mut recipe = chat_recipe();
    recipe.params = vec![
        text_param(),
        n_declared(),
        Param {
            name: "more".into(),
            ..text_param()
        },
    ];
    recipe.steps.insert(3, typed("{more}"));
    approved_recipe(&rig, &recipe);
    let (ok, result) = run_json(
        &rig,
        &recipe.id,
        json!({"text": "4111 1111", "more": "1111 1111", "n": 2}),
    );
    assert!(!ok);
    assert!(
        result["reason"].as_str().unwrap().contains("card number"),
        "{result}"
    );
    assert_eq!(typed_calls(&rig).len(), 1);
    let mut form = chat_recipe();
    form.params = recipe.params.clone();
    form.steps[2] = Step::Fill {
        fields: vec![
            FormField {
                target: target("textbox", "Message"),
                kind: FieldKind::Textbox,
                value: "{text}".into(),
            },
            FormField {
                target: target("textbox", "Message"),
                kind: FieldKind::Textbox,
                value: "{more}".into(),
            },
        ],
        expect: vec![Expect::TextVisible("x".into())],
    };
    let rig = chat_rig(BrowserTrust::Autonomous);
    approved_recipe(&rig, &form);
    let (ok, result) = run_json(
        &rig,
        &form.id,
        json!({"text": "4111 1111", "more": "1111 1111", "n": 2}),
    );
    assert!(!ok);
    assert!(
        result["reason"].as_str().unwrap().contains("card number"),
        "{result}"
    );
    assert!(!rig.web.names().iter().any(|n| n == "browser_fill_form"));
}

#[test]
fn i3_money_words_amounts_and_form_fields_are_classified() {
    for name in [
        "Place your order",
        "Order now",
        "Book now",
        "Reserve",
        "Bid",
        "Sell",
        "Trade",
        "Rent",
        "Send money",
        "Send $50",
        "Send 20 USD",
    ] {
        assert_eq!(
            risk::name_class(name).map(|c| c.risk),
            Some(Risk::Money),
            "{name}"
        );
    }
    assert_ne!(
        risk::name_class("Sort order").map(|c| c.risk),
        Some(Risk::Money)
    );
    assert_eq!(risk::name_class("Send").map(|c| c.risk), Some(Risk::Send));
    // A menu or a toggle of a form can be the payment; a text field is where text goes.
    let select = Step::Select {
        target: target("combobox", "Pay by card"),
        values: vec!["{text}".into()],
        expect: vec![Expect::TextVisible("x".into())],
    };
    assert_eq!(select.risk(), Risk::Money);
    let fill = Step::Fill {
        fields: vec![FormField {
            target: target("checkbox", "Buy now"),
            kind: FieldKind::Checkbox,
            value: "true".into(),
        }],
        expect: vec![Expect::TextVisible("x".into())],
    };
    assert_eq!(fill.risk(), Risk::Money);
    assert_eq!(risk::label_class("combobox", "Subscribe").risk, Risk::Money);
}

#[test]
fn i4_a_typed_parameter_has_no_default_in_a_file_or_a_recording() {
    let mut r = chat_recipe();
    r.params[0].default = Some(json!("hi"));
    assert!(r.validate().unwrap_err().contains("no default"));
    let rig = chat_rig(BrowserTrust::AskRisky);
    mono_chats(&rig);
    mono_sends(&rig);
    let params = json!([
        {"name": "text", "example": TYPED, "default": "hi"},
        {"name": "n", "type": "integer", "default": 2, "example": "2"},
    ]);
    let result = rig.call("recipe_record", chat_draft(params));
    assert_eq!(result["isError"], true);
    assert!(text(&result).contains("no default"), "{}", text(&result));
}

#[test]
fn m1_a_payment_approval_attaches_to_the_click_not_the_typing_after_it() {
    let rig = chat_rig(BrowserTrust::AskRisky);
    rig.call("browser_navigate", json!({"url": CHAT}));
    rig.call("browser_snapshot", json!({}));
    let (handle, _) = user(rig.attach(), vec![true]);
    rig.call(
        "browser_request_approval",
        json!({"category": "payment", "action": "pay"}),
    );
    handle.join().unwrap();
    rig.call(
        "browser_type",
        json!({"target": "e30", "element": "Message", "text": TYPED}),
    );
    mono_sends(&rig);
    let (handle, _) = user(rig.attach(), vec![true]);
    let result = rig.call("recipe_record", chat_draft(say_params()));
    handle.join().unwrap();
    assert_eq!(result["isError"], false, "{}", text(&result));
    let recipe = rig.store().list().remove(0);
    assert!(
        matches!(
            &recipe.steps[1],
            Step::Approve {
                category: ApproveKind::Other
            }
        ),
        "{:?}",
        recipe.steps
    );
    assert!(
        matches!(
            &recipe.steps[3],
            Step::Approve {
                category: ApproveKind::Payment
            }
        ),
        "{:?}",
        recipe.steps
    );
}

#[test]
fn m2_scrub_removes_json_and_javascript_escaped_values() {
    let value = "say \"hi\"\nit's é".to_owned();
    let secrets = [value.clone()];
    let echoes = [
        format!(
            "fill('{}')",
            value
                .replace('\'', "\\'")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        ),
        serde_json::to_string(&value).unwrap(),
        "say \\u0022hi\\u0022\\u000ait\\u0027s \\u00e9".to_owned(),
        value.clone(),
    ];
    for echo in echoes {
        let out = risk::scrub(&echo, &secrets);
        assert!(
            out.contains("[value hidden]") || !out.contains("hi"),
            "{echo} -> {out}"
        );
    }
    let json = serde_json::to_string(&value).unwrap();
    assert!(!risk::scrub(&json, &secrets).contains("say"));
    assert!(!risk::scrub("it\\'s", &["it's".into()]).contains("it"));
}

#[test]
fn m3_the_repair_card_shows_the_locator_diff_and_a_send_keeps_its_area_and_a_name() {
    let mut base = chat_recipe();
    base.steps[4] = Step::Send {
        target: Target {
            candidates: vec![Candidate {
                scope: Some(Scope {
                    role: "navigation".into(),
                    name: "Chat".into(),
                }),
                ..locator("button", "Send")
            }],
        },
        action: SendAction::Send,
        expect: vec![Expect::UrlMatches("x".into())],
    };
    let scoped = |scope: Option<(&str, &str)>, name: Option<&str>| {
        let mut c = Candidate {
            role: Some("button".into()),
            name: name.map(str::to_owned),
            ..Candidate::default()
        };
        c.scope = scope.map(|(r, n)| Scope {
            role: r.into(),
            name: n.into(),
        });
        Patch::ReplaceCandidates {
            candidates: vec![c],
        }
    };
    assert!(repair::apply(
        &base,
        4,
        &scoped(Some(("navigation", "Chat")), Some("send")),
        2
    )
    .is_ok());
    assert!(repair::apply(&base, 4, &scoped(None, Some("Send")), 2).is_err());
    assert!(repair::apply(&base, 4, &scoped(Some(("main", "Other")), Some("Send")), 2).is_err());
    assert!(repair::apply(&base, 4, &scoped(Some(("navigation", "Chat")), None), 2).is_err());
    let applied = repair::apply(
        &base,
        4,
        &scoped(Some(("navigation", "Chat")), Some("send")),
        2,
    )
    .unwrap();
    let diff = repair::locator_diff(&base.steps[4], &applied.recipe.steps[4]);
    assert!(
        diff.contains("name \"Send\"")
            && diff.contains("name \"send\"")
            && diff.contains("inside navigation \"Chat\""),
        "{diff}"
    );
}

#[test]
fn m4_verified_does_not_survive_a_change_of_content() {
    let rig = chat_rig(BrowserTrust::Autonomous);
    let recipe = chat_recipe();
    approved_recipe(&rig, &recipe);
    let db = rig.approvals();
    assert!(db.is_verified(MONO, &recipe.id, &recipe.content_hash()));
    // Edited on disk, then approved again by hash: its stats still say verified, for the old hash.
    let mut edited = recipe.clone();
    edited.description = "Say it, edited".into();
    rig.store().save(&edited).unwrap();
    db.approve_recipe(MONO, &edited.id, &edited.content_hash())
        .unwrap();
    assert!(rig.store().get(&recipe.id).unwrap().stats.verified);
    assert!(!db.is_verified(MONO, &edited.id, &edited.content_hash()));
    let (handle, shown) = user(rig.attach(), vec![false]);
    let (ok, _) = run_json(&rig, &recipe.id, say());
    handle.join().unwrap();
    assert!(!ok);
    assert!(shown.lock().unwrap()[0].detail().contains("first time"));
    assert!(rig.web.tool_calls().is_empty());
}

#[test]
fn r6_a_file_that_claims_to_be_verified_does_not_skip_the_first_run_card() {
    // The stats of the file say verified, the database has no such run: the card is shown.
    let rig = chat_rig(BrowserTrust::Autonomous);
    let recipe = chat_recipe();
    assert!(recipe.stats.verified);
    rig.store().save(&recipe).unwrap();
    rig.approvals()
        .approve_recipe(MONO, &recipe.id, &recipe.content_hash())
        .unwrap();
    let (handle, shown) = user(rig.attach(), vec![false]);
    let (ok, _) = run_json(&rig, &recipe.id, say());
    handle.join().unwrap();
    assert!(!ok);
    assert!(shown.lock().unwrap()[0].detail().contains("first time"));
    // A passing run is remembered for this hash only, and a Mono's data removal forgets it.
    let (handle, _) = user(rig.attach(), vec![true]);
    assert!(run_json(&rig, &recipe.id, say()).0);
    handle.join().unwrap();
    let db = rig.approvals();
    assert!(db.is_verified(MONO, &recipe.id, &recipe.content_hash()));
    assert!(!db.is_verified(MONO, &recipe.id, "sha256:other"));
    db.delete_mono(MONO).unwrap();
    assert!(!db.is_verified(MONO, &recipe.id, &recipe.content_hash()));
}

#[test]
fn r1_a_live_upgraded_card_also_lists_what_was_typed_before_it() {
    // Recorded as a plain write ("Save draft"), the page now says "Send now": the card for the
    // live send lists the text typed before it, though the step itself was recorded as a write.
    let mut recipe = chat_recipe();
    recipe.steps = vec![
        chat_open(),
        approve(ApproveKind::Other),
        typed("{text}"),
        Step::Act {
            target: target("button", "Save draft"),
            expect: vec![Expect::UrlMatches("^https://news\\.example/archive".into())],
        },
        extract_step(),
    ];
    recipe.risk = Risk::Write;
    assert!(recipe.validate().is_ok(), "{:?}", recipe.validate());
    let web = chat_web();
    {
        let mut pages = web.pages.lock().unwrap();
        let page = pages.get_mut(CHAT).unwrap();
        let probe = "- button \"Send now\" [ref=e32]".to_owned();
        page.selectors
            .insert(SAVE_BTN.into(), (probe.clone(), probe));
        page.clicks.insert(SAVE_BTN.into(), ARCHIVE.into());
    }
    let host = Capture {
        inner: ScriptHost::new(web, BrowserTrust::AskRisky),
        cards: Mutex::new(vec![]),
    };
    let outcome = runner::run(&host, &recipe, json!(say()).as_object().unwrap(), fast());
    assert!(outcome.ok(), "{:?}", outcome.failure);
    let cards = host.cards.lock().unwrap().clone();
    let live = cards.last().unwrap();
    assert_eq!(live["category"], "send_message");
    let details = live["details"].as_str().unwrap();
    assert!(
        details.contains("sent with it") && details.contains(TYPED),
        "{details}"
    );
}

#[test]
fn r2_a_card_number_hides_in_longer_digit_runs_slices_and_earlier_fields() {
    let gate = crate::mono::browser::gate::digits_hold_card;
    assert!(gate("order 99 4111111111111111 99"));
    assert!(gate("994111111111111111"));
    assert!(gate("4111_1111_1111_1111"));
    assert!(!gate("12 34 56"));
    // Any contiguous slice of the values, not only the ones from a start to the end.
    assert!(risk::split_card(&["pad", "4111 1111", "1111 1111", "tail"]));
    assert!(!risk::split_card(&["x", "4111", "y", "z"]));
    assert!(risk::split_card(&["4111", "1111", "1111", "1111"]));
    // A card half typed, half in a later select or fill, is refused at the step that completes it.
    let rig = chat_rig(BrowserTrust::Autonomous);
    let mut recipe = chat_recipe();
    recipe.params = vec![
        text_param(),
        n_declared(),
        Param {
            name: "more".into(),
            ..text_param()
        },
    ];
    recipe.steps.insert(
        3,
        Step::Fill {
            fields: vec![FormField {
                target: target("textbox", "Message"),
                kind: FieldKind::Textbox,
                value: "{more}".into(),
            }],
            expect: vec![Expect::TextVisible("x".into())],
        },
    );
    approved_recipe(&rig, &recipe);
    let (ok, result) = run_json(
        &rig,
        &recipe.id,
        json!({"text": "4111 1111", "more": "1111 1111", "n": 2}),
    );
    assert!(!ok);
    assert!(
        result["reason"].as_str().unwrap().contains("card number"),
        "{result}"
    );
    assert!(!rig.web.names().iter().any(|n| n == "browser_fill_form"));
    // The same for a select after a type.
    let mut sel = chat_recipe();
    sel.params = recipe.params.clone();
    sel.steps.insert(
        3,
        Step::Select {
            target: target("combobox", "Channel"),
            values: vec!["{more}".into()],
            expect: vec![Expect::TextVisible("x".into())],
        },
    );
    let rig = chat_rig(BrowserTrust::Autonomous);
    approved_recipe(&rig, &sel);
    let (ok, _) = run_json(
        &rig,
        &sel.id,
        json!({"text": "4111 1111", "more": "1111 1111", "n": 2}),
    );
    assert!(!ok);
    assert!(!rig.web.names().iter().any(|n| n == "browser_select_option"));
}

#[test]
fn r3_order_is_money_except_in_the_phrases_sort_order_and_order_by() {
    let money = |n: &str| risk::name_class(n).map(|c| c.risk) == Some(Risk::Money);
    assert!(!money("Sort order"));
    assert!(!money("Order by date"));
    assert!(money("Order now by card"));
    assert!(money("Place your order"));
    assert!(money("Sort order, then place your order"));
    assert!(money("Standing order"));
}

#[test]
fn r5_a_select_takes_the_class_of_the_live_menu() {
    let mut recipe = chat_recipe();
    recipe.steps = vec![
        chat_open(),
        approve(ApproveKind::Other),
        Step::Select {
            target: target("combobox", "Channel"),
            values: vec!["{text}".into()],
            expect: vec![Expect::RolePresent {
                role: "combobox".into(),
                name: None,
            }],
        },
        extract_step(),
    ];
    recipe.params = vec![text_param(), n_declared()];
    recipe.risk = Risk::Write;
    assert!(recipe.validate().is_ok(), "{:?}", recipe.validate());
    let rig = chat_rig(BrowserTrust::Autonomous);
    {
        let mut pages = rig.web.pages.lock().unwrap();
        let page = pages.get_mut(CHAT).unwrap();
        let probe = "- combobox \"Pay now\" [ref=e35]".to_owned();
        page.selectors
            .insert(CHANNEL_BOX.into(), (probe.clone(), probe.clone()));
        page.selectors
            .insert("internal:role=combobox".into(), (probe.clone(), probe));
    }
    approved_recipe(&rig, &recipe);
    let (handle, shown) = user(rig.attach(), vec![false]);
    let (ok, _) = run_json(&rig, &recipe.id, say());
    handle.join().unwrap();
    assert!(!ok);
    assert_eq!(shown.lock().unwrap()[0].category, Category::Payment);
    assert!(!rig.web.names().iter().any(|n| n == "browser_select_option"));
}
