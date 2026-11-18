use super::*;
use crate::mobile::policy::{filter_reply, Permission};
use serde_json::json;

fn device(kind: DeviceKind) -> DeviceContext {
    DeviceContext {
        id: "d".into(),
        kind,
        permission: Permission::Control,
        projects: None,
        monos: None,
    }
}

fn catalog() -> Value {
    json!({
        "projects": [
            {"id": "local", "name": "Local"},
            {"id": "remote%3A%2F%2Fbox%2Fwork", "name": "Box", "webOnly": true},
        ],
        "monos": [],
        "sessions": [
            {"id": "a", "projectId": "local", "target": {"kind": "local"}},
            {"id": "b", "projectId": "local", "target": {"kind": "codespace", "instance": "x", "state": "stopped"},
             "cloudState": "stopped", "webOnly": true},
            {"id": "c", "projectId": "local", "target": {"kind": "codespace", "instance": "y", "state": "starting"},
             "cloudState": "starting", "startup": {"step": "create"}},
        ],
    })
}

#[test]
fn a_phone_gets_no_web_only_entries_and_no_web_fields() {
    let mut reply = catalog();
    scope_catalog(&device(DeviceKind::Phone), &mut reply);
    assert_eq!(
        reply,
        json!({
            "projects": [{"id": "local", "name": "Local"}],
            "monos": [],
            "sessions": [{"id": "a", "projectId": "local"}, {"id": "c", "projectId": "local"}],
        })
    );
}

#[test]
fn a_web_device_keeps_everything_but_the_marker() {
    let mut reply = catalog();
    scope_catalog(&device(DeviceKind::Web), &mut reply);
    assert_eq!(reply["projects"].as_array().unwrap().len(), 2);
    assert_eq!(reply["sessions"].as_array().unwrap().len(), 3);
    assert!(!reply.to_string().contains("webOnly"));
    assert_eq!(reply["sessions"][1]["cloudState"], "stopped");
    assert_eq!(reply["sessions"][2]["startup"], json!({"step": "create"}));
}

#[test]
fn scope_leaves_other_replies_alone() {
    // The cloud.stop reply names `sessions` with other keys; a read has none.
    let stop = json!({"sessions": [{"sessionId": "s", "title": "t", "busy": false, "target": 1}]});
    let mut reply = stop.clone();
    scope_catalog(&device(DeviceKind::Phone), &mut reply);
    assert_eq!(reply, stop);
    let read = json!({"id": "c", "turns": [], "cloud": {"kind": "codespace"}});
    let mut reply = read.clone();
    scope_catalog(&device(DeviceKind::Phone), &mut reply);
    assert_eq!(reply, read);
}

#[test]
fn filter_reply_applies_the_cloud_scope_after_the_selection() {
    let mut web = device(DeviceKind::Web);
    web.projects = Some(vec!["local".into()]);
    let mut reply = catalog();
    filter_reply(&web, "workspace.list", &mut reply);
    // The remote project is not selected, the cloud session of `local` is.
    assert_eq!(reply["projects"].as_array().unwrap().len(), 1);
    assert_eq!(reply["sessions"].as_array().unwrap().len(), 3);
    web.projects = Some(vec!["other".into()]);
    let mut reply = catalog();
    filter_reply(&web, "sessions.list", &mut reply);
    assert!(reply["sessions"].as_array().unwrap().is_empty());
}

#[test]
fn non_local_projects_are_recognised_by_their_encoded_prefix() {
    for id in [
        "remote%3A%2F%2Fbox%2Fwork",
        "remote%3a%2f%2fbox%2fwork",
        "remote%3A%2f%2Fbox",
        "REMOTE%3A%2F%2Fbox",
        "remote://box/work",
        "remote%3A%2F%2F",
        "%72emote%3A%2F%2Fbox",
    ] {
        assert!(is_non_local_project(id), "{id}");
    }
    for id in [
        "",
        "%2Fhome%2Fme%2Fremote%3A%2F%2Fbox",
        "%2Fhome%2Fme%2Fremote-app",
        "%2Fhome%2Fremote",
        "remote-app",
        "remote",
        "remote%3A",
        "C%3A%2FUsers%2Fme%2Fremote%3A%2F%2F",
        "remote%3Z%2F%2Fbox",
        "%",
        "%E2%82",
    ] {
        assert!(!is_non_local_project(id), "{id}");
    }
}

fn at(base: Instant, seconds: u64) -> Instant {
    base + Duration::from_secs(seconds)
}

#[test]
fn the_hourly_limit_counts_calls_and_rolls() {
    let base = Instant::now();
    let mut limiter = WakeLimiter::new();
    for i in 0..WAKE_LIMIT_PER_HOUR as u64 {
        let slot = limiter.acquire("d", at(base, i)).unwrap();
        limiter.release(&slot);
    }
    assert_eq!(limiter.acquire("d", at(base, 10)), Err(Code::Busy));
    // Other devices are unaffected.
    assert!(limiter.acquire("other", at(base, 10)).is_ok());
    // The oldest call leaves the window after an hour.
    assert_eq!(limiter.acquire("d", at(base, 3599)), Err(Code::Busy));
    assert!(limiter.acquire("d", at(base, 3600)).is_ok());
    assert_eq!(
        limiter.calls_this_hour("d", at(base, 3600)),
        WAKE_LIMIT_PER_HOUR
    );
}

#[test]
fn concurrent_slots_are_released_explicitly_or_by_timeout() {
    let base = Instant::now();
    let mut limiter = WakeLimiter::new();
    let first = limiter.acquire("d", base).unwrap();
    let second = limiter.acquire("d", base).unwrap();
    assert_eq!(limiter.in_flight("d", base), WAKE_CONCURRENT);
    assert_eq!(limiter.acquire("d", at(base, 1)), Err(Code::Busy));
    limiter.release(&first);
    limiter.release(&first); // twice is a no-op
    assert_eq!(limiter.in_flight("d", at(base, 2)), 1);
    let third = limiter.acquire("d", at(base, 2)).unwrap();
    assert_ne!(second, third);
    // A lost completion frees the slot after the timeout.
    let late = at(base, WAKE_SLOT_TIMEOUT.as_secs());
    assert_eq!(
        limiter.in_flight("d", late),
        1,
        "only the newer slot is left"
    );
    assert!(limiter.acquire("d", late).is_ok());
}

#[test]
fn a_refund_frees_the_slot_and_the_count() {
    let base = Instant::now();
    let mut limiter = WakeLimiter::new();
    let slot = limiter.acquire("d", base).unwrap();
    limiter.refund(&slot);
    assert_eq!(limiter.in_flight("d", base), 0);
    assert_eq!(limiter.calls_this_hour("d", base), 0);
}

#[test]
fn reads_have_their_own_limit_per_minute() {
    let base = Instant::now();
    let mut limiter = WakeLimiter::new();
    for _ in 0..READ_LIMIT_PER_MINUTE {
        limiter.allow_read("d", base).unwrap();
    }
    assert_eq!(limiter.allow_read("d", at(base, 59)), Err(Code::Busy));
    assert!(limiter.allow_read("d", at(base, 60)).is_ok());
    // Reads never use wake slots.
    assert_eq!(limiter.in_flight("d", base), 0);
    assert!(limiter.acquire("d", base).is_ok());
    limiter.forget("d");
    assert_eq!(limiter.calls_this_hour("d", base), 0);
}

#[test]
fn the_process_wide_limiter_answers_busy_as_a_coded_failure() {
    // Unique device ids keep this test independent of others that share it.
    let device = "process-wide-limiter-test";
    let slots: Vec<_> = (0..WAKE_CONCURRENT)
        .map(|_| acquire_wake(device).unwrap())
        .collect();
    let refused = acquire_wake(device).unwrap_err();
    assert_eq!(refused, Code::Busy.failure());
    release_wake(&slots[0]);
    let again = acquire_wake(device).unwrap();
    release_wake(&again);
    limiter().forget(device);
}

#[test]
fn startup_errors_lose_paths_urls_commands_and_tokens() {
    let fake_token = concat!("gh", "p_", "0123456789abcdefghij0123456789");
    let cases = [
        ("Codespace quota exceeded", "Codespace quota exceeded"),
        ("failed to read /home/me/work/app/.env", "failed to read"),
        (
            "could not clone https://github.com/o/r.git",
            "could not clone",
        ),
        (
            "ssh connection to user@10.0.0.5 refused",
            "ssh connection to refused",
        ),
        ("C:\\Users\\me\\repo is missing", "is missing"),
        ("GH_TOKEN=abc123 was rejected", "was rejected"),
        ("run gh codespace create --repo o/r failed", "run"),
        ("   too\n many\t spaces  ", "too many spaces"),
    ];
    for (raw, expected) in cases {
        assert_eq!(sanitize_startup_error(raw), expected, "{raw}");
    }
    let token = format!("the credential {fake_token} expired");
    assert_eq!(sanitize_startup_error(&token), "the credential expired");
    assert_eq!(
        sanitize_startup_error("/a/b https://x.y"),
        "The start failed"
    );
    assert_eq!(sanitize_startup_error(""), "The start failed");
}

#[test]
fn startup_errors_are_capped() {
    let long = "word ".repeat(100);
    let out = sanitize_startup_error(&long);
    assert_eq!(out.chars().count(), STARTUP_ERROR_MAX_CHARS);
    assert!(out.ends_with("..."));
    let multibyte = "\u{e9}".repeat(300);
    assert_eq!(
        sanitize_startup_error(&multibyte).chars().count(),
        STARTUP_ERROR_MAX_CHARS
    );
    let exact = "a".repeat(STARTUP_ERROR_MAX_CHARS);
    assert_eq!(sanitize_startup_error(&exact), exact);
}

#[test]
fn a_start_target_accepts_only_the_allowlist() {
    let ok = validate_start_target(&json!({
        "kind": "codespace", "instance": "new", "branch": "main", "machine": "basicLinux32gb"
    }))
    .unwrap();
    assert_eq!(ok.kind, StartKind::Codespace);
    assert_eq!(ok.instance, StartInstance::New);
    assert_eq!(ok.branch.as_deref(), Some("main"));
    assert_eq!(ok.machine.as_deref(), Some("basicLinux32gb"));
    let sandbox =
        validate_start_target(&json!({"kind": "sandbox", "instance": "box-1", "size": "small"}))
            .unwrap();
    assert_eq!(sandbox.instance, StartInstance::Named("box-1".into()));
    assert_eq!(sandbox.size.as_deref(), Some("small"));
    assert_eq!(
        validate_start_target(&json!({"kind": "sandbox"}))
            .unwrap()
            .instance,
        StartInstance::New
    );
}

#[test]
fn a_start_target_refuses_everything_else_as_forbidden() {
    for key in [
        "kit", "template", "image", "cpus", "memory", "ttl", "account", "extra",
    ] {
        let mut target = json!({"kind": "sandbox"});
        target[key] = json!("x");
        assert_eq!(
            validate_start_target(&target),
            Err(Code::Forbidden),
            "{key}"
        );
    }
    for bad in [
        json!(null),
        json!("codespace"),
        json!([]),
        json!({}),
        json!({"kind": "local"}),
        json!({"kind": "remote"}),
        json!({"kind": 1}),
        json!({"kind": "codespace", "instance": 3}),
        json!({"kind": "codespace", "instance": ""}),
        json!({"kind": "codespace", "branch": ["main"]}),
        json!({"kind": "codespace", "branch": "bad\nbranch"}),
        json!({"kind": "codespace", "machine": "m".repeat(201)}),
        json!({"kind": "codespace", "size": "small"}),
        json!({"kind": "sandbox", "machine": "basicLinux32gb"}),
    ] {
        assert_eq!(validate_start_target(&bad), Err(Code::Forbidden), "{bad}");
    }
}
