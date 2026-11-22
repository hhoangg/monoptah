//! Phase 4 tests: export and import, the shared library, the Browser tab's list and dry run,
//! and the polish items of the earlier phases (parameter shapes against the Playwright MCP
//! tool schemas, the streak after a repaired pass, money words, card numbers).
//! Same scripted [`Web`] as the other recipe tests: no real Chrome.
use super::phase3::{
    chat_open, chat_recipe, chat_rig, extract_step, n_declared, target, text_param, typed, CHAT,
    TYPED,
};
use super::*;
use crate::mono::browser::gate::{contains_card_number, digits_hold_card};
use crate::mono::browser::recipe::manage::{self, Status};
use crate::mono::browser::recipe::risk;
use crate::mono::browser::recipe::share::{self, Library, SOURCE_IMPORT, SOURCE_SHARED};

// ---- Parameter shapes against @playwright/mcp 0.0.86 ----

/// A JSON schema reduced to what the Playwright MCP tool schemas use (zod objects): types,
/// required keys, enums and arrays. `closed` objects also refuse a key the schema does not list.
fn conforms(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let kind = schema["type"].as_str().unwrap_or("any");
    let ok = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        _ => true,
    };
    if !ok {
        return Err(format!("{path}: expected {kind}, got {value}"));
    }
    if let Some(options) = schema["enum"].as_array() {
        if !options.contains(value) {
            return Err(format!("{path}: {value} is not one of {options:?}"));
        }
    }
    if kind == "object" {
        let props = schema["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for key in schema["required"].as_array().into_iter().flatten() {
            let key = key.as_str().unwrap();
            if value.get(key).is_none() {
                return Err(format!("{path}: missing `{key}`"));
            }
        }
        for (key, inner) in value.as_object().unwrap() {
            let Some(inner_schema) = props.get(key) else {
                return Err(format!("{path}: unexpected key `{key}`"));
            };
            conforms(inner, inner_schema, &format!("{path}.{key}"))?;
        }
    }
    if kind == "array" {
        for (i, item) in value.as_array().unwrap().iter().enumerate() {
            conforms(item, &schema["items"], &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

/// Transcribed from `playwright-core` 1.66.0-alpha (the engine of `@playwright/mcp` 0.0.86),
/// `tools/backend/snapshot.ts` (`elementSchema`, `selectOptionSchema`), `form.ts` and `type`
/// (`typeSchema`): `target` is required, `element` is optional, and every value is a string.
fn playwright_schema(tool: &str) -> Value {
    let element = json!({"element": {"type": "string"}, "target": {"type": "string"}});
    let with = |extra: Value| {
        let mut props = element.clone();
        props
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        props
    };
    match tool {
        "browser_type" => json!({
            "type": "object", "required": ["target", "text"],
            "properties": with(json!({
                "text": {"type": "string"}, "submit": {"type": "boolean"}, "slowly": {"type": "boolean"},
            })),
        }),
        "browser_select_option" => json!({
            "type": "object", "required": ["target", "values"],
            "properties": with(json!({"values": {"type": "array", "items": {"type": "string"}}})),
        }),
        "browser_fill_form" => json!({
            "type": "object", "required": ["fields"],
            "properties": {"fields": {"type": "array", "items": {
                "type": "object", "required": ["target", "name", "type", "value"],
                "properties": with(json!({
                    "name": {"type": "string"},
                    "type": {"type": "string", "enum": ["textbox", "checkbox", "radio", "combobox", "slider"]},
                    "value": {"type": "string"},
                })),
            }}},
        }),
        other => panic!("no schema for {other}"),
    }
}

const NOTIFY_BOX: &str = "internal:role=checkbox[name=\"Notify\"s]";

#[test]
fn a_write_recipe_sends_the_parameter_shapes_of_the_playwright_mcp_0_0_86_tools() {
    let web = super::phase3::chat_web();
    {
        let mut pages = web.pages.lock().unwrap();
        let page = pages.get_mut(CHAT).unwrap();
        page.body.push_str("  - checkbox \"Notify\" [ref=e36]\n");
        page.selectors.insert(
            NOTIFY_BOX.into(),
            (
                "- checkbox \"Notify\" [ref=e36]".into(),
                "- checkbox \"Notify\" [ref=e36]".into(),
            ),
        );
    }
    let rig = Rig::with(BrowserTrust::Autonomous, web);
    let mut recipe = chat_recipe();
    recipe.params = vec![
        text_param(),
        Param {
            name: "pick".into(),
            ..text_param()
        },
        Param {
            name: "more".into(),
            ..text_param()
        },
        n_declared(),
    ];
    let field = |name: &str, role: &str, kind: FieldKind, value: &str| FormField {
        target: target(role, name),
        kind,
        value: value.into(),
    };
    recipe.steps = vec![
        chat_open(),
        super::phase3::approve(ApproveKind::Other),
        typed("{text}"),
        Step::Fill {
            fields: vec![
                field("Notify", "checkbox", FieldKind::Checkbox, "true"),
                field("Message", "textbox", FieldKind::Textbox, "{more}"),
                field("Channel", "combobox", FieldKind::Combobox, "{pick}"),
            ],
            expect: vec![Expect::TextVisible("x".into())],
        },
        Step::Select {
            target: target("combobox", "Channel"),
            values: vec!["{pick}".into()],
            expect: vec![Expect::TextVisible("x".into())],
        },
        super::phase3::approve(ApproveKind::SendMessage),
        super::phase3::send_button(),
        extract_step(),
    ];
    recipe.risk = Risk::Send;
    recipe.validate().unwrap();
    recipe.stats.verified = true;
    approved_recipe(&rig, &recipe);
    let (ok, result) = run_json(
        &rig,
        &recipe.id,
        json!({"text": TYPED, "more": "second note", "pick": "general", "n": 2}),
    );
    assert!(ok, "{result}");
    let mut seen = Vec::new();
    for (name, args) in rig.web.tool_calls() {
        if ["browser_type", "browser_fill_form", "browser_select_option"].contains(&name.as_str()) {
            conforms(&args, &playwright_schema(&name), &name)
                .unwrap_or_else(|e| panic!("{name} does not match the schema: {e}\n{args}"));
            seen.push((name, args));
        }
    }
    assert_eq!(seen.len(), 3, "{seen:?}");
    // The regression: a checkbox goes as the string "true", never a boolean.
    let fill = &seen[1].1["fields"];
    assert_eq!(fill[0]["type"], "checkbox");
    assert_eq!(fill[0]["value"], "true");
    // The checker itself is not vacuous.
    assert!(conforms(
        &json!({"fields": [{"name": "n", "type": "checkbox", "target": "e1", "value": true}]}),
        &playwright_schema("browser_fill_form"),
        "x"
    )
    .is_err());
    assert!(conforms(
        &json!({"values": ["a"]}),
        &playwright_schema("browser_select_option"),
        "x"
    )
    .is_err());
}

// ---- Polish of Phase 2 and 3 ----

#[test]
fn a_repaired_pass_extends_the_streak_and_recomputes_the_flaky_mark() {
    let mut stats = Stats {
        window: [vec![false; 7], vec![true; 13]].concat(),
        flaky: true,
        consecutive_ok: 2,
        ..Stats::default()
    };
    stats.note_repaired_pass();
    assert_eq!(stats.consecutive_ok, 3);
    assert!(
        !stats.flaky,
        "three passes in a row and the fail rate is back to 30 percent"
    );
    assert_eq!((stats.runs, stats.ok), (1, 1));
    assert!(stats.last_run_ms > 0 && stats.last_ok_ms == stats.last_run_ms);
    // A pass does not hide a bad window: too many failures still set the mark.
    let mut stats = Stats {
        window: vec![true, true, true, false, false],
        ..Stats::default()
    };
    stats.note_repaired_pass();
    assert!(stats.flaky && stats.consecutive_ok == 1);
    assert_eq!(stats.window.len(), 6);
}

#[test]
fn order_words_are_money_not_write() {
    for name in [
        "Order",
        "ORDER",
        "Order #1234",
        "Order now",
        "Reorder",
        "Start ordering",
        "Place order",
    ] {
        assert_eq!(
            risk::name_class(name).map(|c| c.risk),
            Some(Risk::Money),
            "{name}"
        );
        assert_eq!(
            risk::label_class("button", name).risk,
            Risk::Money,
            "{name}"
        );
        let act = Step::Act {
            target: target("button", name),
            expect: vec![Expect::TextVisible("x".into())],
        };
        assert_eq!(act.risk(), Risk::Money, "{name}");
    }
    // Sorting is not buying.
    for name in ["Sort order", "Order by date"] {
        assert_ne!(
            risk::name_class(name).map(|c| c.risk),
            Some(Risk::Money),
            "{name}"
        );
    }
}

#[test]
fn order_and_tracking_numbers_are_not_cards_but_real_test_cards_are() {
    // Passes the Luhn check by chance but is no card brand's number (a leading 7, 0 or 8, or a
    // length no brand issues): an order or tracking number.
    for number in [
        "7000000123456787",
        "71000000000004",
        "01234567890128",
        "8700000000000006",
        // USPS style tracking number: 22 digits, and a stretch of it passes Luhn.
        "9400728378326616630494",
        "9400152758929007540091",
    ] {
        assert!(!digits_hold_card(number), "{number}");
        assert!(!contains_card_number(
            &json!({"q": format!("track {number}")})
        ));
        assert!(risk::secret_value(number).is_none(), "{number}");
    }
    // The standard test numbers of every major brand are still cards, alone, with separators,
    // with text around them and glued to other digits.
    for card in [
        "4111111111111111",
        "5555555555554444",
        "378282246310005",
        "6011111111111117",
        "3530111333300000",
        "30569309025904",
        "6200000000000005",
        "2223003122003222",
        "9704000000000018",
        "6759649826438453",
    ] {
        assert!(digits_hold_card(card), "{card}");
        assert!(
            contains_card_number(&json!({"q": format!("pay with {card} now")})),
            "{card}"
        );
        assert!(digits_hold_card(&format!("99{card}99")), "glued {card}");
        let grouped: Vec<String> = card
            .as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        assert!(digits_hold_card(&grouped.join(" ")), "{card}");
        assert!(risk::secret_value(card).is_some(), "{card}");
    }
}

// ---- Rigs ----

fn import_into(rig: &Rig, text: &str, blocked: &[String]) -> Result<Recipe, String> {
    share::import(
        &rig.store(),
        &rig.approvals(),
        MONO,
        text,
        blocked,
        SOURCE_IMPORT,
    )
}
fn approve_current(rig: &Rig, id: &str) {
    let hash = rig.store().get(id).unwrap().content_hash();
    manage::approve(&rig.store(), &rig.approvals(), MONO, id, &hash).unwrap();
}
/// The export text of an approved recipe of a first Mono.
fn exported(recipe: &Recipe) -> String {
    let producer = Rig::new(BrowserTrust::AskRisky);
    approved_recipe(&producer, recipe);
    share::export(&producer.store().get(&recipe.id).unwrap()).unwrap()
}
fn edit(text: &str, change: impl FnOnce(&mut Value)) -> String {
    let mut value: Value = serde_json::from_str(text).unwrap();
    change(&mut value);
    serde_json::to_string(&value).unwrap()
}

// ---- Export and import ----

#[test]
fn an_export_has_a_version_origin_steps_and_hash_and_no_stats() {
    let mut recipe = sample_recipe();
    recipe.stats.runs = 9;
    recipe.stats.ok = 7;
    recipe.stats.window = vec![true, false];
    let text = exported(&recipe);
    let file: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(file["format"], "monoptah-recipe");
    assert_eq!(file["format_version"], 1);
    assert_eq!(file["origin"], SITE);
    assert_eq!(file["version"], 1);
    assert_eq!(file["recipe"]["steps"].as_array().unwrap().len(), 2);
    let hash = file["hash"].as_str().unwrap();
    assert!(hash.starts_with("sha256:") && hash.len() == 71, "{hash}");
    let mut bare = sample_recipe();
    bare.params.iter_mut().for_each(|p| p.default = None);
    assert_eq!(hash, bare.content_hash());
    // Nothing of the runs, and no id to impersonate: the stats are the defaults.
    let stats = &file["recipe"]["stats"];
    assert_eq!(stats["runs"], 0);
    assert_eq!(stats["window"], json!([]));
    assert!(!text.contains("window\": [\n        true"));
    // It reads back (the export has no secrets and nothing unknown).
    let parsed = share::parse(&text, &[]).unwrap();
    assert_eq!(parsed.recipe.steps, sample_recipe().steps);
    assert!(parsed.recipe.params.iter().all(|p| p.default.is_none()));
}

#[test]
fn an_imported_recipe_is_a_draft_with_a_new_id_and_needs_approval_then_a_dry_run() {
    let text = exported(&sample_recipe());
    let rig = Rig::new(BrowserTrust::AskRisky);
    let recipe = import_into(&rig, &text, &[]).unwrap();
    assert_ne!(recipe.id, sample_recipe().id, "a new id in the new Mono");
    assert_eq!(recipe.version, 1);
    assert_eq!(recipe.stats.runs, 0);
    let revision = recipe.stats.revision.clone().unwrap();
    assert_eq!(revision.reason, "import");
    // Saved, unapproved, and marked as coming from outside.
    assert_eq!(rig.store().list().len(), 1);
    assert!(!rig.approvals().recipe_approved(MONO, &recipe));
    assert_eq!(
        rig.approvals().outside_source(MONO, &recipe.id).as_deref(),
        Some("import")
    );
    let rows = manage::rows(&rig.store(), &rig.approvals(), MONO);
    assert_eq!(rows[0].status, Status::Draft);
    assert_eq!(rows[0].outside.as_deref(), Some("import"));
    // 1. A draft does not run, and neither a Mono's find nor its list lies about it.
    let (ok, _) = run_json(&rig, &recipe.id, json!({"n": 2}));
    assert!(!ok);
    let list = json_of(&rig.call("recipe_list", json!({})));
    assert_eq!(list["recipes"][0]["status"], "draft");
    // A dry run needs the approval first: it opens the recipe's pages in the browser.
    let err = rig.gate.dry_run_recipe(&recipe.id).unwrap_err();
    assert!(err.contains("Approve"), "{err}");
    assert!(rig.web.names().is_empty(), "nothing touched the browser");
    // 2. Approved, but not yet dry run: still refused, with the reason.
    approve_current(&rig, &recipe.id);
    let result = rig.call("recipe_run", json!({"id": recipe.id, "params": {"n": 2}}));
    assert_eq!(result["isError"], true);
    assert!(text_of(&result).contains("dry run"), "{}", text_of(&result));
    let list = json_of(&rig.call("recipe_list", json!({})));
    assert_eq!(list["recipes"][0]["status"], "needs_dry_run");
    assert!(rig.web.names().is_empty());
    // 3. A dry run that fails does not unlock it.
    rename_list(&rig, "stories");
    let report = rig.gate.dry_run_recipe(&recipe.id).unwrap();
    assert!(!report.ok && report.failure.is_some(), "{report:?}");
    assert!(!run_json(&rig, &recipe.id, json!({"n": 2})).0);
    assert_eq!(
        manage::rows(&rig.store(), &rig.approvals(), MONO)[0].status,
        Status::NeedsDryRun
    );
    // 4. One that passes does, for exactly that content.
    rename_list(&rig, "Stories");
    let report = rig.gate.dry_run_recipe(&recipe.id).unwrap();
    assert!(report.ok, "{report:?}");
    assert!(run_json(&rig, &recipe.id, json!({"n": 2})).0);
    assert_eq!(
        manage::rows(&rig.store(), &rig.approvals(), MONO)[0].status,
        Status::Approved
    );
    // A file edited after that no longer matches the approval: it does not run at all.
    let path = rig.store().path_of(&recipe);
    let mut edited: Recipe = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edited.description = "Read other stories".into();
    std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
    assert!(!run_json(&rig, &recipe.id, json!({"n": 2})).0);
    // A recipe that came from outside never loses its mark by a file edit.
    assert!(rig.approvals().outside_source(MONO, &recipe.id).is_some());
}

fn text_of(result: &Value) -> String {
    text(result)
}

#[test]
fn an_import_refuses_tampered_unknown_secret_blocked_newer_and_oversized_files() {
    let good = exported(&sample_recipe());
    let rig = Rig::new(BrowserTrust::AskRisky);
    let refuses = |text: &str, blocked: &[String], expect: &str| {
        let err = import_into(&rig, text, blocked).unwrap_err();
        assert!(
            err.to_lowercase().contains(&expect.to_lowercase()),
            "wanted `{expect}` in `{err}`"
        );
        assert!(rig.store().list().is_empty(), "nothing was saved: {err}");
    };
    // Tamper: a step edited after the export, a hash that does not match.
    refuses(
        &edit(&good, |v| {
            v["recipe"]["steps"][0]["url"] = json!("https://news.example/evil");
        }),
        &[],
        "hash",
    );
    refuses(
        &edit(&good, |v| {
            v["hash"] = json!(format!("sha256:{}", "0".repeat(64)))
        }),
        &[],
        "hash",
    );
    // A file that disagrees with itself.
    refuses(
        &edit(&good, |v| v["origin"] = json!("https://other.example")),
        &[],
        "disagrees",
    );
    // An unknown step kind (a code step, say) is refused, never skipped.
    refuses(
        &edit(&good, |v| {
            v["recipe"]["steps"][1] = json!({"op": "evaluate", "code": "alert(1)"});
        }),
        &[],
        "step kind",
    );
    // An unknown field, and a wrong or newer format.
    refuses(&edit(&good, |v| v["extra"] = json!(1)), &[], "malformed");
    refuses(
        &edit(&good, |v| v["format_version"] = json!(2)),
        &[],
        "newer",
    );
    refuses(
        &edit(&good, |v| v["format"] = json!("x")),
        &[],
        "not a monoptah",
    );
    refuses("{not json", &[], "not a recipe");
    refuses(&"x".repeat(share::MAX_FILE_BYTES + 1), &[], "too big");
    // A secret in the file: a key shape in the description (built, so no realistic token is
    // in this source), whatever the hash says.
    let key = ["gh", "p_", &"A1b2".repeat(6)].concat();
    let mut secret = sample_recipe();
    secret.description = format!("Use {key}");
    let hash = secret.content_hash();
    refuses(
        &edit(&good, |v| {
            v["recipe"] = serde_json::to_value(&secret).unwrap();
            v["hash"] = json!(hash);
        }),
        &[],
        "key or token",
    );
    let mut card = sample_recipe();
    card.params[0].kind = ParamKind::String;
    card.params[0].default = Some(json!("4111111111111111"));
    let hash = card.content_hash();
    refuses(
        &edit(&good, |v| {
            v["recipe"] = serde_json::to_value(&card).unwrap();
            v["hash"] = json!(hash);
        }),
        &[],
        "card",
    );
    // A site this Mono blocks, and a bank or payment site, are refused.
    refuses(&good, &[SITE.to_owned()], "blocked");
    let mut bank = sample_recipe();
    bank.origin = "https://secure.mybank.example".into();
    bank.steps[0] = Step::Open {
        url: "https://secure.mybank.example/archive".into(),
        expect: vec![Expect::UrlMatches(
            "^https://secure\\.mybank\\.example/archive".into(),
        )],
    };
    let hash = bank.content_hash();
    let bank_file = edit(&good, |v| {
        v["origin"] = json!(bank.origin);
        v["recipe"] = serde_json::to_value(&bank).unwrap();
        v["hash"] = json!(hash);
    });
    refuses(&bank_file, &[], "bank");
    // The good file is fine, once; the same intent is not brought in twice.
    import_into(&rig, &good, &[]).unwrap();
    let err = import_into(&rig, &good, &[]).unwrap_err();
    assert!(err.contains("already has a recipe"), "{err}");
    assert_eq!(rig.store().list().len(), 1);
}

#[test]
fn a_file_with_stats_or_a_foreign_id_brings_neither_in() {
    let good = exported(&sample_recipe());
    let rig = Rig::new(BrowserTrust::AskRisky);
    let loaded = edit(&good, |v| {
        v["recipe"]["stats"]["runs"] = json!(500);
        v["recipe"]["stats"]["ok"] = json!(500);
        v["recipe"]["stats"]["verified"] = json!(true);
    });
    let recipe = import_into(&rig, &loaded, &[]).unwrap();
    assert_eq!((recipe.stats.runs, recipe.stats.ok), (0, 0));
    assert!(!recipe.stats.verified);
    assert_ne!(recipe.id, "rcp_0a1b2c3d");
    // `verified` of a write recipe is only in the database: a file can not claim it.
    assert!(!rig
        .approvals()
        .is_verified(MONO, &recipe.id, &recipe.content_hash()));
}

#[test]
fn export_refuses_a_recipe_that_holds_a_secret() {
    let mut recipe = sample_recipe();
    recipe.description = format!("token {}", ["gh", "p_", &"Z9y8".repeat(6)].concat());
    let err = share::export(&recipe).unwrap_err();
    assert!(err.contains("key or token"), "{err}");
    let mut recipe = sample_recipe();
    recipe.params[0].description = "pay 4111 1111 1111 1111".into();
    assert!(share::export(&recipe).unwrap_err().contains("card"));
    // A recipe that is not valid is not exported either.
    let mut recipe = sample_recipe();
    recipe.risk = Risk::Money;
    assert!(share::export(&recipe).is_err());
}

#[test]
fn a_write_recipe_dry_run_never_acts_and_the_recipe_still_asks_on_its_first_run() {
    let text = exported(&chat_recipe());
    let rig = chat_rig(BrowserTrust::AskRisky);
    let recipe = import_into(&rig, &text, &[]).unwrap();
    assert!(recipe.changes_things());
    approve_current(&rig, &recipe.id);
    let report = rig.gate.dry_run_recipe(&recipe.id).unwrap();
    assert!(report.ok, "{report:?}");
    // It stopped before the first `approve` step (step 2) and typed nothing, clicked no button.
    assert_eq!(report.stopped_before, Some(2));
    let names = rig.web.names();
    let acted = names.iter().filter(|n| {
        matches!(
            n.as_str(),
            "browser_type"
                | "browser_click"
                | "browser_fill_form"
                | "browser_select_option"
                | "browser_press_key"
        )
    });
    assert_eq!(acted.count(), 0, "{names:?}");
    // Dry run counts as a dry run, not as a verified run: the first real run asks the user.
    assert!(!rig
        .approvals()
        .is_verified(MONO, &recipe.id, &recipe.content_hash()));
    assert!(!rig.approvals().needs_dry_run(MONO, &recipe));
    let (handle, shown) = user(rig.attach(), vec![false]);
    let result = rig.call(
        "recipe_run",
        json!({"id": recipe.id, "params": {"text": TYPED, "n": 2}}),
    );
    handle.join().unwrap();
    assert_eq!(result["isError"], true);
    assert_eq!(shown.lock().unwrap().len(), 1, "the first run card");
}

#[test]
fn a_dry_run_obeys_the_blocked_list_and_never_touches_the_browser_for_a_blocked_site() {
    let rig = Rig::new(BrowserTrust::AskRisky);
    let recipe = sample_recipe();
    approved_recipe(&rig, &recipe);
    rig.approvals()
        .mark_outside(MONO, &recipe.id, "import")
        .unwrap();
    rig.gate
        .set_policy(Policy::new(BrowserTrust::AskRisky, vec![SITE.to_owned()]));
    let report = rig.gate.dry_run_recipe(&recipe.id).unwrap();
    assert!(!report.ok, "{report:?}");
    assert!(report.failure.unwrap().contains("blocked"));
    assert!(rig.web.names().is_empty(), "{:?}", rig.web.names());
    assert!(rig.approvals().needs_dry_run(MONO, &recipe));
}

// ---- The shared library ----

fn library() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(dir.path().join("_shared_recipes"));
    (dir, library)
}

#[test]
fn only_an_approved_dry_run_recipe_is_promoted_and_unsharing_leaves_copies_alone() {
    let (_dir, library) = library();
    let rig = Rig::new(BrowserTrust::AskRisky);
    let mut recipe = sample_recipe();
    rig.store().save(&recipe).unwrap();
    // A draft is not shared.
    let err =
        manage::promote(&rig.store(), &rig.approvals(), &library, MONO, &recipe.id).unwrap_err();
    assert!(err.contains("approved"), "{err}");
    approve_current(&rig, &recipe.id);
    manage::promote(&rig.store(), &rig.approvals(), &library, MONO, &recipe.id).unwrap();
    let listed = library.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].shared_by.as_deref(), Some(MONO));
    assert_eq!(listed[0].recipe.stats, Stats::default());
    // The same Mono can share a new version; another Mono can not take the name.
    recipe.version = 2;
    library.promote(MONO, &recipe).unwrap();
    let err = library.promote("mono-2", &recipe).unwrap_err();
    assert!(err.contains("Another Mono"), "{err}");
    // A recipe from outside is not re-shared before it passed its dry run in this Mono.
    let outside = import_into(
        &Rig::new(BrowserTrust::AskRisky),
        &exported(&sample_recipe()),
        &[],
    )
    .unwrap();
    assert_eq!(outside.version, 1);
    let consumer = Rig::new(BrowserTrust::AskRisky);
    let copy = library
        .adopt(
            SITE,
            "news.archive",
            &consumer.store(),
            &consumer.approvals(),
            MONO,
            &[],
        )
        .unwrap();
    approve_current(&consumer, &copy.id);
    let err = manage::promote(
        &consumer.store(),
        &consumer.approvals(),
        &library,
        MONO,
        &copy.id,
    )
    .unwrap_err();
    assert!(err.contains("dry run"), "{err}");
    // Unsharing removes the entry; the copy stays.
    library.unshare(SITE, "news.archive").unwrap();
    assert!(library.list().is_empty());
    assert!(library.unshare(SITE, "news.archive").is_err());
    assert_eq!(consumer.store().list().len(), 1);
    assert!(library.unshare("https://x.example", "../escape").is_err());
}

#[test]
fn a_shared_recipe_is_copied_as_an_unapproved_draft_and_the_consuming_monos_rules_apply() {
    let (_dir, library) = library();
    let source = Rig::new(BrowserTrust::AskRisky);
    let recipe = click_recipe();
    approved_recipe(&source, &recipe);
    manage::promote(
        &source.store(),
        &source.approvals(),
        &library,
        MONO,
        &recipe.id,
    )
    .unwrap();

    // A Mono that blocks the site can not copy it.
    let blocking = Rig::new(BrowserTrust::AskRisky);
    let err = library
        .adopt(
            SITE,
            "news.archive",
            &blocking.store(),
            &blocking.approvals(),
            MONO,
            &[SITE.to_owned()],
        )
        .unwrap_err();
    assert!(err.contains("blocked"), "{err}");
    assert!(blocking.store().list().is_empty());

    // Another Mono copies it: a draft with its own id, marked `shared`, nothing approved.
    let consumer = Rig::new(BrowserTrust::AskRisky);
    let copy = library
        .adopt(
            SITE,
            "news.archive",
            &consumer.store(),
            &consumer.approvals(),
            MONO,
            &[],
        )
        .unwrap();
    assert_ne!(copy.id, recipe.id);
    assert_eq!(
        consumer
            .approvals()
            .outside_source(MONO, &copy.id)
            .as_deref(),
        Some("shared")
    );
    assert_eq!(copy.stats.revision.as_ref().unwrap().reason, SOURCE_SHARED);
    assert!(!consumer.approvals().recipe_approved(MONO, &copy));
    assert!(!run_json(&consumer, &copy.id, json!({"n": 2})).0);
    // The source Mono's approval and dry run do not carry over.
    assert!(!consumer
        .approvals()
        .dry_run_passed(MONO, &copy.id, &copy.content_hash()));
    approve_current(&consumer, &copy.id);
    assert!(
        !run_json(&consumer, &copy.id, json!({"n": 2})).0,
        "needs the dry run"
    );
    let report = consumer.gate.dry_run_recipe(&copy.id).unwrap();
    assert!(report.ok, "{report:?}");
    assert!(run_json(&consumer, &copy.id, json!({"n": 2})).0);

    // Trust level and blocked list are read again at run time: raising the block list stops it.
    consumer
        .gate
        .set_policy(Policy::new(BrowserTrust::AskRisky, vec![SITE.to_owned()]));
    let (ok, result) = run_json(&consumer, &copy.id, json!({"n": 2}));
    assert!(!ok);
    assert!(
        result["reason"].as_str().unwrap().contains("blocked"),
        "{result}"
    );
    // And a Mono at Level 1 can not run a recipe that clicks, shared or not.
    consumer
        .gate
        .set_policy(Policy::new(BrowserTrust::ReadOnly, vec![]));
    let (ok, result) = run_json(&consumer, &copy.id, json!({"n": 2}));
    assert!(!ok);
    assert!(
        result["reason"].as_str().unwrap().contains("Level 1"),
        "{result}"
    );
    let dry = consumer.gate.dry_run_recipe(&copy.id).unwrap();
    assert!(!dry.ok, "{dry:?}");
    // Sharing widened nothing for the Mono that had no rights: a Level 1 Mono that copies it
    // gets the same refusal from its first dry run.
    let weak = Rig::new(BrowserTrust::ReadOnly);
    let weak_copy = library
        .adopt(
            SITE,
            "news.archive",
            &weak.store(),
            &weak.approvals(),
            MONO,
            &[],
        )
        .unwrap();
    approve_current(&weak, &weak_copy.id);
    assert!(!weak.gate.dry_run_recipe(&weak_copy.id).unwrap().ok);
    assert!(!run_json(&weak, &weak_copy.id, json!({"n": 2})).0);
}

#[test]
fn a_library_file_edited_by_hand_is_skipped_and_never_adopted() {
    let (_dir, library) = library();
    let source = Rig::new(BrowserTrust::AskRisky);
    let recipe = sample_recipe();
    approved_recipe(&source, &recipe);
    manage::promote(
        &source.store(),
        &source.approvals(),
        &library,
        MONO,
        &recipe.id,
    )
    .unwrap();
    let file = std::fs::read_dir(library.dir())
        .unwrap()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten())
        .map(|e| e.path())
        .next()
        .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        edit(&text, |v| {
            v["recipe"]["steps"][0]["url"] = json!("https://news.example/evil");
        }),
    )
    .unwrap();
    assert!(library.list().is_empty());
    let consumer = Rig::new(BrowserTrust::AskRisky);
    let err = library
        .adopt(
            SITE,
            "news.archive",
            &consumer.store(),
            &consumer.approvals(),
            MONO,
            &[],
        )
        .unwrap_err();
    assert!(err.contains("hash"), "{err}");
    assert!(consumer.store().list().is_empty());
}

// ---- The Browser tab's list and actions ----

#[test]
fn the_list_shows_status_runs_success_rate_last_run_flaky_and_the_steps_without_values() {
    let rig = chat_rig(BrowserTrust::Autonomous);
    let mut recipe = chat_recipe();
    recipe.stats.verified = true;
    approved_recipe(&rig, &recipe);
    rig.store().update_stats(&recipe, |s| {
        s.runs = 10;
        s.ok = 8;
        s.last_run_ms = 1_700_000_000_000;
        s.flaky = true;
        s.repaired = true;
    });
    let rows = manage::rows(&rig.store(), &rig.approvals(), MONO);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        (row.intent.as_str(), row.origin.as_str(), row.version),
        ("chat.say", SITE, 1)
    );
    assert_eq!(row.status, Status::Approved);
    assert_eq!(row.success_percent(), Some(80));
    assert_eq!(row.last_run_ms, 1_700_000_000_000);
    assert!(row.flaky && row.repaired);
    assert_eq!(row.risk, Risk::Send);
    assert!(row.outside.is_none() && row.previous.is_none());
    // A recipe that never ran has no rate.
    let fresh = Rig::new(BrowserTrust::ReadOnly);
    approved_recipe(&fresh, &sample_recipe());
    assert_eq!(
        manage::rows(&fresh.store(), &fresh.approvals(), MONO)[0].success_percent(),
        None
    );
    // The steps are in words, with placeholders; a value typed in some run is nowhere.
    let steps = manage::steps_text(&recipe);
    assert!(steps.contains("{text}"), "{steps}");
    assert!(steps.contains("Risk: send"));
    assert!(!steps.contains(TYPED));
    let (_ok, _) = run_json(&rig, &recipe.id, json!({"text": TYPED, "n": 2}));
    let stored = std::fs::read_to_string(rig.store().path_of(&recipe)).unwrap();
    assert!(
        !stored.contains(TYPED)
            && !manage::steps_text(&rig.store().get(&recipe.id).unwrap()).contains(TYPED)
    );
}

#[test]
fn approve_is_bound_to_the_content_shown_and_rollback_and_delete_keep_their_rules() {
    let rig = Rig::new(BrowserTrust::AskRisky);
    let v1 = sample_recipe();
    rig.store().save(&v1).unwrap();
    let shown = v1.content_hash();
    // The file changes between the card and the click: nothing is approved.
    let mut changed = v1.clone();
    changed.description = "Read something else".into();
    rig.store().save(&changed).unwrap();
    let err = manage::approve(&rig.store(), &rig.approvals(), MONO, &v1.id, &shown).unwrap_err();
    assert!(err.contains("changed"), "{err}");
    assert!(rig.approvals().approved_hash(MONO, &v1.id).is_none());
    // Back to what was shown, approve it, then a second version.
    let rig = Rig::new(BrowserTrust::AskRisky);
    rig.store().save(&v1).unwrap();
    manage::approve(&rig.store(), &rig.approvals(), MONO, &v1.id, &shown).unwrap();
    let mut v2 = v1.clone();
    v2.version = 2;
    v2.description = "Read the stories, version two".into();
    rig.store().save(&v2).unwrap();
    let row = &manage::rows(&rig.store(), &rig.approvals(), MONO)[0];
    assert_eq!(
        (row.version, row.previous, row.status),
        (2, Some(1), Status::Draft)
    );
    // Rolling back to the approved content is approved again; to unapproved content it is not.
    let restored = manage::rollback(&rig.store(), &rig.approvals(), MONO, &v1.id, 1).unwrap();
    assert_eq!(restored.version, 1);
    assert_eq!(
        manage::rows(&rig.store(), &rig.approvals(), MONO)[0].status,
        Status::Approved
    );
    manage::rollback(&rig.store(), &rig.approvals(), MONO, &v1.id, 2).unwrap();
    assert_eq!(
        manage::rows(&rig.store(), &rig.approvals(), MONO)[0].status,
        Status::Draft
    );
    assert!(manage::rollback(&rig.store(), &rig.approvals(), MONO, &v1.id, 9).is_err());
    // Delete removes the file, the history and every approval and mark.
    rig.approvals()
        .mark_outside(MONO, &v1.id, "import")
        .unwrap();
    rig.approvals().mark_dry_run(MONO, &v1.id, &shown).unwrap();
    manage::delete(&rig.store(), &rig.approvals(), MONO, &v1.id).unwrap();
    assert!(rig.store().list().is_empty());
    assert!(rig.store().history(SITE, "news.archive").is_empty());
    assert!(rig.approvals().approved_hash(MONO, &v1.id).is_none());
    assert!(rig.approvals().outside_source(MONO, &v1.id).is_none());
    assert!(!rig.approvals().dry_run_passed(MONO, &v1.id, &shown));
    assert!(!rig.approvals().hash_was_approved(MONO, &v1.id, &shown));
    assert!(manage::delete(&rig.store(), &rig.approvals(), MONO, &v1.id).is_err());
}

#[test]
fn a_repair_of_an_imported_recipe_keeps_it_runnable_and_counts_the_pass_in_the_streak() {
    let text = exported(&sample_recipe());
    let rig = Rig::new(BrowserTrust::AskRisky);
    let recipe = import_into(&rig, &text, &[]).unwrap();
    approve_current(&rig, &recipe.id);
    assert!(rig.gate.dry_run_recipe(&recipe.id).unwrap().ok);
    rename_list(&rig, "stories");
    let failed = rig.call("recipe_run", json!({"id": recipe.id, "params": {"n": 2}}));
    let token = token_of(&json_of(&failed));
    let (ok, fixed) = repair_with(&rig, &token, top_stories_patch());
    assert!(ok, "{fixed}");
    let current = rig.store().get(&recipe.id).unwrap();
    assert_eq!(current.version, 2);
    // The live run that passed is its dry run: it keeps working, and it is still marked outside.
    assert!(rig
        .approvals()
        .dry_run_passed(MONO, &current.id, &current.content_hash()));
    assert!(rig.approvals().outside_source(MONO, &current.id).is_some());
    assert_eq!(current.stats.consecutive_ok, 1);
    assert!(run_json(&rig, &recipe.id, json!({"n": 2})).0);
}

// ---- Review fixes ----

#[test]
fn export_and_the_library_never_carry_a_parameter_default_and_import_drops_one() {
    let mut recipe = sample_recipe();
    recipe.params.push(Param {
        name: "who".into(),
        kind: ParamKind::String,
        description: "a person".into(),
        default: Some(json!("lan.nguyen@example.com")),
    });
    recipe.steps[0] = Step::Open {
        url: "https://news.example/archive?u={who}".into(),
        expect: vec![Expect::UrlMatches("^https://news\\.example/archive".into())],
    };
    recipe.validate().unwrap();
    let text = share::export(&recipe).unwrap();
    assert!(
        !text.contains("lan.nguyen") && !text.contains("\"default\""),
        "{text}"
    );
    let (_dir, library) = library();
    library.promote(MONO, &recipe).unwrap();
    let entry = std::fs::read_dir(library.dir())
        .unwrap()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten())
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .next()
        .unwrap();
    assert!(!entry.contains("lan.nguyen") && !entry.contains("\"default\""));
    // A hand made file with a default (and a matching hash) loses it on import.
    let mut with = share::parse(&text, &[]).unwrap().recipe;
    with.params[1].default = Some(json!("somebody"));
    let hash = with.content_hash();
    let file = edit(&text, |v| {
        v["recipe"] = serde_json::to_value(&with).unwrap();
        v["hash"] = json!(hash);
    });
    let rig = Rig::new(BrowserTrust::AskRisky);
    let imported = import_into(&rig, &file, &[]).unwrap();
    assert!(imported.params.iter().all(|p| p.default.is_none()));
    assert!(!std::fs::read_to_string(rig.store().path_of(&imported))
        .unwrap()
        .contains("somebody"));
}

#[test]
fn two_origins_with_the_same_readable_name_do_not_share_a_library_slot() {
    let (_dir, library) = library();
    let mk = |origin: &str| {
        let mut r = sample_recipe();
        r.origin = origin.into();
        r.steps[0] = Step::Open {
            url: format!("{origin}/archive"),
            expect: vec![Expect::UrlMatches("^https://".into())],
        };
        r
    };
    let (a, b) = (mk("https://my-site.com"), mk("https://my.site.com"));
    library.promote(MONO, &a).unwrap();
    library.promote(MONO, &b).unwrap();
    assert_eq!(library.list().len(), 2);
    library
        .unshare("https://my-site.com", "news.archive")
        .unwrap();
    let left = library.list();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].origin, "https://my.site.com");
    // A file moved to another slot is not the entry asked for.
    let rig = Rig::new(BrowserTrust::AskRisky);
    library.promote(MONO, &a).unwrap();
    let from = library
        .dir()
        .read_dir()
        .unwrap()
        .flatten()
        .map(|d| d.path())
        .collect::<Vec<_>>();
    assert_eq!(from.len(), 2);
    let err = library
        .adopt(
            "https://elsewhere.example",
            "news.archive",
            &rig.store(),
            &rig.approvals(),
            MONO,
            &[],
        )
        .unwrap_err();
    assert!(err.contains("not shared"), "{err}");
}

#[test]
fn recording_over_an_outside_recipe_counts_as_its_dry_run() {
    let text = exported(&sample_recipe());
    let rig = Rig::new(BrowserTrust::AskRisky);
    let old = import_into(&rig, &text, &[]).unwrap();
    approve_current(&rig, &old.id);
    assert!(rig.gate.dry_run_recipe(&old.id).unwrap().ok);
    browse_archive(&rig);
    let (handle, _) = user(rig.attach(), vec![true]);
    let result = rig.call("recipe_record", draft(n_param()));
    handle.join().unwrap();
    assert_eq!(result["isError"], false, "{}", super::text(&result));
    let now = rig.store().get(&old.id).unwrap();
    assert_eq!(now.version, 2);
    assert!(rig.approvals().outside_source(MONO, &now.id).is_some());
    assert!(rig
        .approvals()
        .dry_run_passed(MONO, &now.id, &now.content_hash()));
    assert!(!rig.approvals().needs_dry_run(MONO, &now));
}

#[test]
fn jcb_15_digit_and_every_visa_length_are_still_cards() {
    fn complete(prefix: &str) -> String {
        (0..10)
            .map(|c| format!("{prefix}{c}"))
            .find(|n| {
                n.bytes()
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
                    .sum::<u32>()
                    % 10
                    == 0
            })
            .unwrap()
    }
    for (prefix, len) in [
        ("2131", 15),
        ("1800", 15),
        ("4", 13),
        ("4", 14),
        ("4", 15),
        ("4", 17),
        ("4", 18),
        ("4", 19),
    ] {
        let number = complete(&format!("{prefix}{}", "5".repeat(len - 1 - prefix.len())));
        assert_eq!(number.len(), len);
        assert!(digits_hold_card(&number), "{number}");
    }
}
