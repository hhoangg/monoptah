//! `device.selection` through the host: the web device's list of everything
//! published, narrowing, and widening with a server ticket.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const VERSION_CALL: &str = "device.selection";
const SECRET_PATH: &str = "/Users/someone/secret-client/alpha";
const OTHER_PATH: &str = "/Users/someone/secret-client/beta";

fn open() -> (tempfile::TempDir, MobileHost) {
    let dir = tempfile::tempdir().unwrap();
    let (host, _events) = MobileHost::open(dir.path().join("mobile-access.json")).unwrap();
    host.set_flags_for_test(true, true);
    host.publish(json!({
        "projects": [
            {"id": SECRET_PATH, "name": "Alpha", "extra": "leak-1"},
            {"id": OTHER_PATH, "name": "Beta\u{7}\n"},
            {"id": "/Users/someone/unnamed"},
        ],
        "monos": [{"id": "mono-1", "name": "Helper", "path": "/Users/someone/mono"}],
        "sessions": [],
    }))
    .unwrap();
    (dir, host)
}

fn row(kind: DeviceKind, permission: Permission, fingerprint: &str) -> Device {
    Device {
        id: format!("id-{fingerprint}"),
        name: "Work laptop".into(),
        fingerprint: fingerprint.into(),
        push: None,
        kind,
        permission,
        projects: Some(vec![SECRET_PATH.into()]),
        monos: Some(vec![]),
        server_device_id: (kind == DeviceKind::Web).then(|| "srv-1".to_owned()),
        created_at: None,
        last_seen: None,
    }
}

fn request(params: Value) -> Value {
    json!({"version": VERSION, "id": "s", "method": VERSION_CALL, "params": params})
}

async fn web(host: &MobileHost, peer: &str, params: Value) -> Result<Value, Failure> {
    tokio::time::timeout(
        Duration::from_secs(10),
        host.handle_web(peer, request(params)),
    )
    .await
    .expect("the selection call must not hang")
}

fn code(result: Result<Value, Failure>) -> String {
    result.unwrap_err().code.unwrap_or_default()
}

fn ref_of(reply: &Value, kind: &str, name: &str) -> String {
    reply["available"][kind]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not listed: {reply}"))["ref"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn row_of(host: &MobileHost, fingerprint: &str) -> Device {
    host.inner
        .lock()
        .unwrap()
        .config
        .devices
        .iter()
        .find(|device| device.fingerprint == fingerprint)
        .cloned()
        .unwrap()
}

#[tokio::test]
async fn unshared_items_carry_a_name_and_a_ref_but_never_an_id_or_path() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Read, "w-read"));
    let reply = web(&host, "w-read", json!({})).await.unwrap();
    assert_eq!(reply["projects"], json!([SECRET_PATH]));
    assert_eq!(reply["canWiden"], false);
    let projects = reply["available"]["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 3, "every published project, in order");
    assert_eq!(projects[0]["name"], "Alpha");
    assert_eq!(projects[0]["shared"], true);
    assert_eq!(projects[0]["id"], SECRET_PATH);
    assert_eq!(
        projects[1]["name"], "Beta",
        "control characters are stripped"
    );
    assert_eq!(projects[1]["shared"], false);
    assert!(projects[1].get("id").is_none());
    assert_eq!(projects[2]["name"], "Unnamed project", "never the id");
    let refs: Vec<_> = projects
        .iter()
        .map(|p| p["ref"].as_str().unwrap())
        .collect();
    assert!(refs
        .iter()
        .all(|r| r.len() == 32 && r.chars().all(|c| c.is_ascii_hexdigit())));
    assert!(refs.iter().collect::<std::collections::HashSet<_>>().len() == 3);
    // No other field of the catalog entry is copied, and no path of an
    // unshared item appears anywhere in the serialized reply.
    let text = reply.to_string();
    assert!(!text.contains(OTHER_PATH), "{text}");
    assert!(!text.contains("/Users/someone/unnamed"), "{text}");
    assert!(!text.contains("/Users/someone/mono"), "{text}");
    assert!(!text.contains("leak-1"), "{text}");
    assert_eq!(reply["available"]["monos"][0]["name"], "Helper");
    assert_eq!(reply["available"]["monos"][0]["shared"], false);
    assert!(reply["available"]["monos"][0].get("id").is_none());
}

#[tokio::test]
async fn the_ref_of_a_project_differs_from_the_ref_of_a_mono_with_the_same_id() {
    let secret = [7u8; 32];
    use super::super::selection::{make_ref, Kind};
    assert_ne!(
        make_ref(&secret, Kind::Project, "same"),
        make_ref(&secret, Kind::Mono, "same")
    );
    assert_ne!(
        make_ref(&secret, Kind::Project, "same"),
        make_ref(&[8u8; 32], Kind::Project, "same")
    );
}

#[tokio::test]
async fn a_phone_reply_and_rules_are_unchanged() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Phone, Permission::Control, "phone-c"));
    host.add_device_for_test(row(DeviceKind::Phone, Permission::Read, "phone-r"));
    let reply = host.handle("phone-c", request(json!({}))).await.unwrap();
    assert_eq!(reply, json!({"projects": [SECRET_PATH], "monos": []}));
    // Narrowing needs Control, widening is forbidden, as before.
    let narrowed = host
        .handle("phone-c", request(json!({"projects": []})))
        .await
        .unwrap();
    assert_eq!(narrowed, json!({"projects": [], "monos": []}));
    let widen = host
        .handle(
            "phone-c",
            request(json!({"projects": [SECRET_PATH, OTHER_PATH], "ticket": "t"})),
        )
        .await
        .unwrap_err();
    assert_eq!(widen, policy::Code::Forbidden.message());
    // The web fields mean nothing to a phone.
    let ignored = host
        .handle(
            "phone-c",
            request(json!({"addProjects": ["x"], "ticket": "t"})),
        )
        .await
        .unwrap();
    assert_eq!(ignored, json!({"projects": [], "monos": []}));
    let denied = host
        .handle("phone-r", request(json!({"projects": []})))
        .await
        .unwrap_err();
    assert_eq!(denied, policy::Code::ReadOnly.message());
    assert!(host.handle("phone-r", request(json!({}))).await.is_ok());
}

#[tokio::test]
async fn a_read_web_device_may_narrow_but_never_widen() {
    let (_dir, host) = open();
    let mut reader = row(DeviceKind::Web, Permission::Read, "w-read");
    reader.projects = Some(vec![SECRET_PATH.into(), OTHER_PATH.into()]);
    host.add_device_for_test(reader);
    host.inner.lock().unwrap().watches.insert(
        "watch".into(),
        Watcher {
            peer: "w-read".into(),
            via: DeviceKind::Web,
            sink: tokio::sync::mpsc::channel::<Value>(1).0,
        },
    );
    let reply = web(&host, "w-read", json!({"projects": [SECRET_PATH]}))
        .await
        .unwrap();
    assert_eq!(reply["projects"], json!([SECRET_PATH]));
    assert_eq!(
        row_of(&host, "w-read").projects,
        Some(vec![SECRET_PATH.into()])
    );
    assert!(host.inner.lock().unwrap().watches.is_empty(), "watches end");
    // Adding by ref, by id or all of them: read_only, nothing changes.
    let beta = ref_of(&reply, "projects", "Beta");
    for params in [
        json!({"addProjects": [beta], "ticket": "t"}),
        json!({"projects": null}),
        json!({"monos": null, "ticket": "t"}),
    ] {
        assert_eq!(
            code(web(&host, "w-read", params.clone()).await),
            "read_only",
            "{params}"
        );
    }
    // A raw id list never adds: it is cut down to what the device has.
    let kept = web(
        &host,
        "w-read",
        json!({"projects": [SECRET_PATH, OTHER_PATH]}),
    )
    .await
    .unwrap();
    assert_eq!(kept["projects"], json!([SECRET_PATH]));
    assert_eq!(
        row_of(&host, "w-read").projects,
        Some(vec![SECRET_PATH.into()])
    );
}

#[tokio::test]
async fn widening_without_a_ticket_needs_a_fresh_passkey() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    host.set_ticket_verifier(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        true
    });
    let reply = web(&host, "w-ctl", json!({})).await.unwrap();
    assert_eq!(reply["canWiden"], true);
    let beta = ref_of(&reply, "projects", "Beta");
    for params in [
        json!({"addProjects": [beta]}),
        json!({"addProjects": [beta], "ticket": ""}),
        json!({"projects": null}),
    ] {
        assert_eq!(
            code(web(&host, "w-ctl", params.clone()).await),
            "fresh_required",
            "{params}"
        );
    }
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the server is not asked");
    assert_eq!(
        row_of(&host, "w-ctl").projects,
        Some(vec![SECRET_PATH.into()])
    );
}

#[tokio::test]
async fn a_ticket_the_server_refuses_changes_nothing() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    host.set_ticket_verifier(move |device, ticket| {
        log.lock().unwrap().push((device, ticket));
        false
    });
    let beta = ref_of(
        &web(&host, "w-ctl", json!({})).await.unwrap(),
        "projects",
        "Beta",
    );
    let failure = web(
        &host,
        "w-ctl",
        json!({"addProjects": [beta], "ticket": "bad"}),
    )
    .await;
    assert_eq!(code(failure), "fresh_required");
    assert_eq!(
        *seen.lock().unwrap(),
        [("srv-1".to_owned(), "bad".to_owned())],
        "the device row's server id and the ticket"
    );
    assert_eq!(
        row_of(&host, "w-ctl").projects,
        Some(vec![SECRET_PATH.into()])
    );
    assert!(host.web_status().access_changes.is_empty());
}

#[tokio::test]
async fn a_row_without_a_server_id_cannot_verify_and_is_refused() {
    let (_dir, host) = open();
    let mut local = row(DeviceKind::Web, Permission::Control, "w-ctl");
    local.server_device_id = None;
    host.add_device_for_test(local);
    host.set_ticket_verifier(|_, _| true);
    let beta = ref_of(
        &web(&host, "w-ctl", json!({})).await.unwrap(),
        "projects",
        "Beta",
    );
    let failure = web(
        &host,
        "w-ctl",
        json!({"addProjects": [beta], "ticket": "t"}),
    )
    .await;
    assert_eq!(code(failure), "fresh_required");
}

#[tokio::test]
async fn a_good_ticket_widens_at_once_and_is_audited_and_shown() {
    let (dir, host) = open();
    let log = dir.path().join("audit");
    let _turn = audit::test_log(&log);
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    host.inner.lock().unwrap().watches.insert(
        "watch".into(),
        Watcher {
            peer: "w-ctl".into(),
            via: DeviceKind::Web,
            sink: tokio::sync::mpsc::channel::<Value>(1).0,
        },
    );
    host.set_ticket_verifier(|_, ticket| ticket == "good");
    let first = web(&host, "w-ctl", json!({})).await.unwrap();
    let (beta, helper) = (
        ref_of(&first, "projects", "Beta"),
        ref_of(&first, "monos", "Helper"),
    );
    let reply = web(
        &host,
        "w-ctl",
        json!({"addProjects": [beta], "addMonos": [helper], "ticket": "good"}),
    )
    .await
    .unwrap();
    assert_eq!(reply["projects"], json!([SECRET_PATH, OTHER_PATH]));
    assert_eq!(reply["monos"], json!(["mono-1"]));
    let projects = reply["available"]["projects"].as_array().unwrap();
    assert_eq!(projects[1]["shared"], true);
    assert_eq!(projects[1]["id"], OTHER_PATH);
    assert_eq!(projects[2]["shared"], false, "the third stays unshared");
    let saved: Value = serde_json::from_slice(&std::fs::read(&host.path).unwrap()).unwrap();
    assert_eq!(
        saved["devices"][0]["projects"],
        json!([SECRET_PATH, OTHER_PATH])
    );
    assert!(host.inner.lock().unwrap().watches.is_empty(), "watches end");
    // The audit line and the access change, each with the names only.
    let lines = audit::read_recent(&log, 50);
    let widen: Vec<_> = lines
        .iter()
        .filter(|line| line["event"] == "selection_widen")
        // The audit log is process-wide: parallel tests write into it too.
        .filter(|line| line["detail"] == "Beta, Helper")
        .collect();
    assert_eq!(widen.len(), 1, "{lines:?}");
    assert_eq!(widen[0]["detail"], "Beta, Helper");
    assert_eq!(widen[0]["transport"], "web");
    let changes = host.web_status().access_changes;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].device_name, "Work laptop");
    assert_eq!(
        changes[0].text,
        "Work laptop now has access to Beta, Helper"
    );
    assert!(!format!("{changes:?}{widen:?}").contains(OTHER_PATH));
    // Pure narrowing writes no widening line and needs no ticket.
    web(&host, "w-ctl", json!({"projects": [SECRET_PATH]}))
        .await
        .unwrap();
    assert_eq!(host.web_status().access_changes.len(), 1);
}

#[tokio::test]
async fn an_unknown_ref_fails_the_whole_call_before_the_server_is_asked() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    host.set_ticket_verifier(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        true
    });
    let reply = web(&host, "w-ctl", json!({})).await.unwrap();
    let beta = ref_of(&reply, "projects", "Beta");
    for params in [
        json!({"addProjects": ["0123456789abcdef0123456789abcdef"], "ticket": "t"}),
        json!({"addProjects": [beta, "nope"], "ticket": "t"}),
        json!({"addMonos": [beta], "ticket": "t"}),
        // A narrowing with a stale ref fails too.
        json!({"projects": [], "addProjects": ["nope"]}),
    ] {
        assert_eq!(
            code(web(&host, "w-ctl", params.clone()).await),
            "not_found",
            "{params}"
        );
    }
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    assert_eq!(
        row_of(&host, "w-ctl").projects,
        Some(vec![SECRET_PATH.into()])
    );
}

#[tokio::test]
async fn malformed_web_fields_are_invalid_parameters() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    let too_many: Vec<String> = (0..1001).map(|i| i.to_string()).collect();
    for params in [
        json!({"addProjects": "x"}),
        json!({"addProjects": [1]}),
        json!({"addMonos": ["x".repeat(65)]}),
        json!({"addProjects": too_many}),
        json!({"ticket": 5}),
        json!({"ticket": "t".repeat(257)}),
    ] {
        let failure = web(&host, "w-ctl", params.clone()).await.unwrap_err();
        assert_eq!(failure.error, "Invalid mobile parameters", "{params}");
    }
}

#[tokio::test]
async fn all_projects_including_new_ones_works_and_new_projects_show_as_shared() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    host.set_ticket_verifier(|_, ticket| ticket == "good");
    let reply = web(
        &host,
        "w-ctl",
        json!({"projects": null, "monos": null, "ticket": "good"}),
    )
    .await
    .unwrap();
    assert_eq!(reply["projects"], Value::Null);
    assert_eq!(reply["monos"], Value::Null);
    assert!(reply["available"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["shared"] == true && item["id"].is_string()));
    let changes = host.web_status().access_changes;
    assert_eq!(
        changes[0].text,
        "Work laptop now has access to all projects, including new ones, all Monos, including new ones"
    );
    // A project published later is shared at once.
    host.publish(json!({
        "projects": [{"id": "/Users/someone/new", "name": "Brand new"}],
        "monos": [],
        "sessions": [],
    }))
    .unwrap();
    let later = web(&host, "w-ctl", json!({})).await.unwrap();
    assert_eq!(later["available"]["projects"][0]["name"], "Brand new");
    assert_eq!(later["available"]["projects"][0]["shared"], true);
    assert_eq!(
        later["available"]["projects"][0]["id"],
        "/Users/someone/new"
    );
    // Asking for "all" again is not a widening and needs no ticket.
    assert!(web(&host, "w-ctl", json!({"projects": null})).await.is_ok());
    // Narrowing from all to a list is allowed with no ticket.
    let narrowed = web(&host, "w-ctl", json!({"projects": ["/Users/someone/new"]}))
        .await
        .unwrap();
    assert_eq!(narrowed["projects"], json!(["/Users/someone/new"]));
}

#[tokio::test]
async fn widening_is_not_rate_limited() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    host.set_ticket_verifier(move |_, ticket| {
        counter.fetch_add(1, Ordering::SeqCst);
        ticket == "good"
    });
    // More attempts than the old limit of five an hour, back to back.
    for round in 0..7 {
        let narrowed = web(&host, "w-ctl", json!({"projects": [SECRET_PATH]}))
            .await
            .unwrap();
        assert_eq!(narrowed["projects"], json!([SECRET_PATH]), "round {round}");
        let widened = web(&host, "w-ctl", json!({"projects": null, "ticket": "good"}))
            .await
            .unwrap_or_else(|failure| panic!("round {round}: {failure:?}"));
        assert_eq!(widened["projects"], Value::Null, "round {round}");
        assert_eq!(row_of(&host, "w-ctl").projects, None, "round {round}");
    }
    assert_eq!(
        asked.load(Ordering::SeqCst),
        7,
        "every attempt reached the server"
    );
}

#[tokio::test]
async fn a_device_revoked_during_the_server_check_is_not_widened() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    let inside = host.clone();
    // The verifier takes the host lock: it would hang if the call held it.
    host.set_ticket_verifier(move |_, _| {
        inside
            .inner
            .lock()
            .unwrap()
            .config
            .devices
            .retain(|device| device.fingerprint != "w-ctl");
        true
    });
    let beta = ref_of(
        &web(&host, "w-ctl", json!({})).await.unwrap(),
        "projects",
        "Beta",
    );
    let failure = web(
        &host,
        "w-ctl",
        json!({"addProjects": [beta], "ticket": "t"}),
    )
    .await;
    assert_eq!(code(failure), "revoked");
    assert!(host.web_status().access_changes.is_empty());
}

#[tokio::test]
async fn a_device_lowered_to_read_during_the_server_check_is_not_widened() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    let inside = host.clone();
    host.set_ticket_verifier(move |_, _| {
        inside.inner.lock().unwrap().config.devices[0].permission = Permission::Read;
        true
    });
    let beta = ref_of(
        &web(&host, "w-ctl", json!({})).await.unwrap(),
        "projects",
        "Beta",
    );
    let failure = web(
        &host,
        "w-ctl",
        json!({"addProjects": [beta], "ticket": "t"}),
    )
    .await;
    assert_eq!(code(failure), "read_only");
    assert_eq!(
        row_of(&host, "w-ctl").projects,
        Some(vec![SECRET_PATH.into()])
    );
    assert!(host.web_status().access_changes.is_empty());
}

#[tokio::test]
async fn the_selection_is_decided_again_from_what_it_is_after_the_server_check() {
    let (_dir, host) = open();
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-ctl"));
    let inside = host.clone();
    // While the server answers, the owner narrows the device to nothing.
    host.set_ticket_verifier(move |_, _| {
        inside.inner.lock().unwrap().config.devices[0].projects = Some(vec![]);
        true
    });
    let beta = ref_of(
        &web(&host, "w-ctl", json!({})).await.unwrap(),
        "projects",
        "Beta",
    );
    let reply = web(
        &host,
        "w-ctl",
        json!({"projects": [SECRET_PATH], "addProjects": [beta], "ticket": "t"}),
    )
    .await
    .unwrap();
    // The owner's narrowing survives: the stale list adds nothing, only the
    // ref does.
    assert_eq!(reply["projects"], json!([OTHER_PATH]));
}

#[tokio::test]
async fn a_raw_id_in_the_list_does_not_widen_even_with_a_valid_ticket() {
    let (dir, host) = open();
    let log = dir.path().join("audit");
    let _turn = audit::test_log(&log);
    host.add_device_for_test(row(DeviceKind::Web, Permission::Control, "w-raw"));
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    host.set_ticket_verifier(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        true
    });
    let reply = web(
        &host,
        "w-raw",
        json!({
            "projects": [SECRET_PATH, OTHER_PATH, "/not/in/the/catalog"],
            "monos": ["mono-1"],
            "ticket": "good",
        }),
    )
    .await
    .unwrap();
    assert_eq!(reply["projects"], json!([SECRET_PATH]));
    assert_eq!(reply["monos"], json!([]));
    assert_eq!(
        row_of(&host, "w-raw").projects,
        Some(vec![SECRET_PATH.into()])
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0, "nothing widens");
    assert!(host.web_status().access_changes.is_empty());
    let lines = audit::read_recent(&log, 50);
    assert!(
        !lines
            .iter()
            .any(|line| line["event"] == "selection_widen" && line["deviceId"] == "id-w-raw"),
        "{lines:?}"
    );
}

#[tokio::test]
async fn narrowing_and_refused_widening_are_audited_without_the_ticket() {
    let (dir, host) = open();
    let log = dir.path().join("audit");
    let _turn = audit::test_log(&log);
    let mut device = row(DeviceKind::Web, Permission::Control, "w-aud");
    device.projects = None;
    host.add_device_for_test(device);
    host.set_ticket_verifier(|_, ticket| ticket == "good");
    web(&host, "w-aud", json!({"projects": [SECRET_PATH]}))
        .await
        .unwrap();
    let beta = ref_of(
        &web(&host, "w-aud", json!({})).await.unwrap(),
        "projects",
        "Beta",
    );
    let secret_ticket = "ticket-value-9f2";
    assert_eq!(
        code(web(&host, "w-aud", json!({"addProjects": [beta.clone()]})).await),
        "fresh_required"
    );
    assert_eq!(
        code(
            web(
                &host,
                "w-aud",
                json!({"addProjects": [beta], "ticket": secret_ticket}),
            )
            .await
        ),
        "fresh_required"
    );
    let lines: Vec<Value> = audit::read_recent(&log, 50)
        .into_iter()
        .filter(|line| line["deviceId"] == "id-w-aud")
        .collect();
    let detail_of = |event: &str| -> Vec<String> {
        lines
            .iter()
            .filter(|line| line["event"] == event)
            .map(|line| line["detail"].as_str().unwrap_or_default().to_owned())
            .collect()
    };
    assert_eq!(detail_of("selection_narrow"), ["projects all -> 1"]);
    assert_eq!(
        // `read_recent` lists the newest first.
        detail_of("selection_widen_refused"),
        ["ticket rejected", "no ticket"]
    );
    assert!(
        !serde_json::to_string(&lines)
            .unwrap()
            .contains(secret_ticket),
        "the ticket is never logged"
    );
}

#[tokio::test]
async fn catalog_refs_change_with_the_host_secret() {
    let (_dir_a, a) = open();
    let (_dir_b, b) = open();
    a.add_device_for_test(row(DeviceKind::Web, Permission::Read, "w"));
    b.add_device_for_test(row(DeviceKind::Web, Permission::Read, "w"));
    let ra = ref_of(&web(&a, "w", json!({})).await.unwrap(), "projects", "Beta");
    let rb = ref_of(&web(&b, "w", json!({})).await.unwrap(), "projects", "Beta");
    assert_ne!(ra, rb, "each host keys its refs with its own secret");
}
