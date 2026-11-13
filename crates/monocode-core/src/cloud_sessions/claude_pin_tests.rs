//! The per-instance Claude account pin: the resolution matrix for both backends, the stored
//! shape (an id, never a secret), and the codespace resolver the app installs.
use super::claude_pin::*;
use super::project_cloud::{self, CodespaceProjectConfig, ProjectCloud};
use crate::provider::Kind;
use crate::remote::codespace_claude::CodespaceClaude;
use crate::store::Store;
use serde_json::json;
use std::path::Path;
use std::sync::{Arc, Mutex};

const TOKEN: &str = "sk-ant-oat01-pin-fixture-token";

fn pin(account: Option<&str>, pinned: bool) -> ClaudePin {
    ClaudePin {
        account: account.map(str::to_owned),
        pinned,
    }
}
fn choice(account: Option<&str>) -> Option<Option<String>> {
    Some(account.map(str::to_owned))
}

#[test]
fn resolution_matrix_is_the_same_rule_for_both_backends() {
    // (pin, project's current choice) -> account
    type Case = (
        Option<ClaudePin>,
        Option<Option<String>>,
        Option<&'static str>,
    );
    let cases: Vec<Case> = vec![
        // A per-session override wins over any project choice, and over "none".
        (Some(pin(Some("a"), true)), choice(Some("b")), Some("a")),
        (Some(pin(Some("a"), true)), choice(None), Some("a")),
        (Some(pin(None, true)), choice(Some("b")), None),
        (Some(pin(Some("a"), true)), None, Some("a")),
        // An unpinned instance follows the project's CURRENT choice.
        (Some(pin(Some("a"), false)), choice(Some("b")), Some("b")),
        (Some(pin(Some("a"), false)), choice(None), None),
        (Some(pin(None, false)), choice(Some("b")), Some("b")),
        // No project known: what the instance recorded.
        (Some(pin(Some("a"), false)), None, Some("a")),
        (Some(pin(None, false)), None, None),
        // A legacy instance (no pin) follows the project when there is one, else nothing.
        (None, choice(Some("b")), Some("b")),
        (None, choice(None), None),
        (None, None, None),
    ];
    for (pin, project, want) in cases {
        assert_eq!(
            effective(pin.as_ref(), project.as_ref()).as_deref(),
            want,
            "{pin:?} / {project:?}"
        );
    }
}

#[test]
fn a_pin_round_trips_holds_only_an_id_and_is_cleared_on_forget() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let key = codespace_key("octo-abc");
    assert_eq!(key, "codespace-claude-account:octo-abc");
    assert_eq!(sandbox_key("box"), "sandbox-claude-account:box");
    assert_eq!(read(&store, &key), None);
    write(&store, &key, &pin(Some("acct-1"), true)).unwrap();
    assert_eq!(read(&store, &key), Some(pin(Some("acct-1"), true)));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&store.setting(&key).unwrap().unwrap()).unwrap(),
        json!({"account": "acct-1", "pinned": true})
    );
    // Forgetting the workspace forgets the pin, for either backend.
    for (cloud, key) in [
        (super::CloudRef::codespace("octo-abc"), key.clone()),
        (super::CloudRef::sandbox("box"), sandbox_key("box")),
    ] {
        write(&store, &key, &pin(None, false)).unwrap();
        store.forget_cloud(&cloud).unwrap();
        assert_eq!(read(&store, &key), None, "{key}");
        // The setting row itself is gone, not left behind as an empty value.
        assert_eq!(store.setting(&key).unwrap(), None, "{key}");
    }
}

/// A store with a data directory, a Claude account with a setup token and one without.
struct Accounts {
    _dir: tempfile::TempDir,
    store: Arc<Mutex<Store>>,
    with_token: String,
    without_token: String,
}
fn accounts() -> Accounts {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("monocode.db")).unwrap();
    let mut book = store.provider_accounts().unwrap();
    let with_token = book.add(Kind::Claude, "Work").unwrap().id;
    let without_token = book.add(Kind::Claude, "Empty").unwrap().id;
    store.save_provider_accounts(&book).unwrap();
    crate::accounts::Profile::new(&with_token, dir.path())
        .save_claude_token(TOKEN)
        .unwrap();
    Accounts {
        _dir: dir,
        store: Arc::new(Mutex::new(store)),
        with_token,
        without_token,
    }
}
fn project_with(store: &Store, cwd: &str, account: Option<&str>) {
    project_cloud::save(
        store,
        cwd,
        &ProjectCloud {
            codespace: CodespaceProjectConfig {
                claude_account: account.map(str::to_owned),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
}
fn bind(store: &Store, cwd: &str, codespace: &str) {
    store
        .remember_cloud_project(
            "env-1",
            &json!({"id": "p1", "cwd": "/workspaces/widgets"}),
            cwd,
            codespace,
        )
        .unwrap();
}

#[test]
fn a_codespaces_account_follows_its_pin_then_its_projects_current_choice() {
    let a = accounts();
    let store = a.store.lock().unwrap();
    bind(&store, "/p", "cs");
    // Legacy: no pin, project without an account.
    assert_eq!(codespace_account(&store, "cs"), None);
    // No pin yet: the project's choice applies.
    project_with(&store, "/p", Some("proj-a"));
    assert_eq!(codespace_account(&store, "cs").as_deref(), Some("proj-a"));
    // Created following the project, then the project changes: the next reconnect applies it.
    write(&store, &codespace_key("cs"), &pin(Some("proj-a"), false)).unwrap();
    project_with(&store, "/p", Some("proj-b"));
    assert_eq!(codespace_account(&store, "cs").as_deref(), Some("proj-b"));
    project_with(&store, "/p", None);
    assert_eq!(codespace_account(&store, "cs"), None);
    // A per-session override stays whatever the project says.
    write(&store, &codespace_key("cs"), &pin(Some("mine"), true)).unwrap();
    project_with(&store, "/p", Some("proj-b"));
    assert_eq!(codespace_account(&store, "cs").as_deref(), Some("mine"));
    // A codespace of no known project uses what it recorded.
    write(&store, &codespace_key("loose"), &pin(Some("x"), false)).unwrap();
    assert_eq!(codespace_account(&store, "loose").as_deref(), Some("x"));
    assert_eq!(codespace_account(&store, "never-seen"), None);
}

#[test]
fn the_resolver_hands_over_the_credential_and_names_an_account_it_cannot_use() {
    let a = accounts();
    let resolver = StoreCodespaceClaude::new(a.store.clone());
    bind(&a.store.lock().unwrap(), "/p", "cs");
    // No account chosen: the Codespaces secret, nothing to deliver.
    assert!(resolver.resolve("cs").unwrap().is_none());
    let set = |account: Option<&str>| {
        project_with(&a.store.lock().unwrap(), "/p", account);
    };
    set(Some(&a.with_token));
    let delivery = resolver.resolve("cs").unwrap().unwrap();
    assert_eq!(delivery.account, a.with_token);
    assert_eq!(delivery.secret.as_str(), TOKEN);
    assert!(delivery.prelude.contains(TOKEN));
    assert!(delivery.prelude.contains("unset ANTHROPIC_AUTH_TOKEN"));
    // An account without a token or key: a clear error naming it, no fallback.
    set(Some(&a.without_token));
    let error = resolver.resolve("cs").err().unwrap();
    assert!(
        error.contains("\"Empty\"") && error.contains("Cloud settings"),
        "{error}"
    );
    // An account that is gone.
    set(Some("account-deleted"));
    let error = resolver.resolve("cs").err().unwrap();
    assert!(
        error.contains("no longer available") && error.contains("Cloud settings"),
        "{error}"
    );
    // No error or label ever carries the token.
    assert!(!error.contains(TOKEN));
    assert_eq!(label(&a.store.lock().unwrap(), &a.with_token), "Work");
}

#[test]
fn the_files_the_pin_writes_never_hold_a_secret() {
    let a = accounts();
    let store = a.store.lock().unwrap();
    write(
        &store,
        &codespace_key("cs"),
        &pin(Some(&a.with_token), true),
    )
    .unwrap();
    let raw = store.setting(&codespace_key("cs")).unwrap().unwrap();
    assert!(!raw.contains(TOKEN), "{raw}");
    assert!(raw.contains(&a.with_token));
}

#[test]
fn a_pinned_codespace_names_its_own_session_when_its_account_is_gone() {
    let a = accounts();
    let resolver = StoreCodespaceClaude::new(a.store.clone());
    bind(&a.store.lock().unwrap(), "/p", "cs");
    let pinned = |account: &str| {
        write(
            &a.store.lock().unwrap(),
            &codespace_key("cs"),
            &pin(Some(account), true),
        )
        .unwrap();
    };
    // Gone: the account belongs to this session, so Cloud settings would not help.
    pinned("account-deleted");
    let error = resolver.resolve("cs").err().unwrap();
    assert!(
        error.starts_with("This cloud session uses Claude account")
            && error.contains("no longer available")
            && error.contains("Start a new cloud session or add the account back")
            && !error.contains("Cloud settings"),
        "{error}"
    );
    assert_eq!(resolver.account_id("cs").err().unwrap(), error);
    // No token: the account is named.
    pinned(&a.without_token);
    let error = resolver.resolve("cs").err().unwrap();
    assert!(
        error.contains("\"Empty\"")
            && error.contains("no setup token or API key")
            && error.contains("Start a new cloud session"),
        "{error}"
    );
    // The project's own choice keeps the Cloud settings wording.
    write(
        &a.store.lock().unwrap(),
        &codespace_key("cs"),
        &pin(Some(&a.with_token), false),
    )
    .unwrap();
    project_with(&a.store.lock().unwrap(), "/p", Some(&a.without_token));
    assert!(resolver
        .resolve("cs")
        .err()
        .unwrap()
        .contains("Cloud settings"));
}

#[test]
fn the_account_id_is_answered_without_reading_the_credential() {
    let a = accounts();
    let resolver = StoreCodespaceClaude::new(a.store.clone());
    bind(&a.store.lock().unwrap(), "/p", "cs");
    project_with(&a.store.lock().unwrap(), "/p", Some(&a.with_token));
    assert_eq!(
        resolver.account_id("cs").unwrap().as_deref(),
        Some(a.with_token.as_str())
    );
    // Damaging the sealed key makes the credential unreadable but not the id check, which
    // looks at the account's metadata only.
    for entry in std::fs::read_dir(a._dir.path()).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains("key") {
            std::fs::write(entry.path(), b"damaged").unwrap();
        }
    }
    assert!(
        resolver.resolve("cs").is_err(),
        "the credential is unreadable"
    );
    assert_eq!(
        resolver.account_id("cs").unwrap().as_deref(),
        Some(a.with_token.as_str())
    );
    // No account chosen: no id.
    project_with(&a.store.lock().unwrap(), "/p", None);
    assert_eq!(resolver.account_id("cs").unwrap(), None);
}

#[test]
fn the_delivered_hint_is_an_id_only_setting_cleared_with_the_instance() {
    let a = accounts();
    let resolver = StoreCodespaceClaude::new(a.store.clone());
    assert_eq!(delivered_key("cs"), "codespace-claude-delivered:cs");
    assert_eq!(resolver.delivered_hint("cs"), None);
    resolver.note_delivered("cs", Some(&a.with_token));
    assert_eq!(
        resolver.delivered_hint("cs").as_deref(),
        Some(a.with_token.as_str())
    );
    // The setting holds the id and nothing else.
    let raw = a
        .store
        .lock()
        .unwrap()
        .setting(&delivered_key("cs"))
        .unwrap()
        .unwrap();
    assert_eq!(raw, a.with_token);
    assert!(!raw.contains(TOKEN));
    // Delivering without an account forgets it.
    resolver.note_delivered("cs", None);
    assert_eq!(resolver.delivered_hint("cs"), None);
    // Forgetting the codespace forgets it too.
    resolver.note_delivered("cs", Some(&a.with_token));
    a.store
        .lock()
        .unwrap()
        .forget_cloud(&super::CloudRef::codespace("cs"))
        .unwrap();
    assert_eq!(resolver.delivered_hint("cs"), None);
}

#[test]
fn a_named_account_resolves_for_a_codespace_whatever_the_project_chooses_now() {
    let a = accounts();
    let resolver = StoreCodespaceClaude::new(a.store.clone());
    bind(&a.store.lock().unwrap(), "/p", "cs");
    project_with(&a.store.lock().unwrap(), "/p", None);
    let delivery = resolver
        .resolve_account("cs", &a.with_token)
        .unwrap()
        .unwrap();
    assert_eq!(delivery.account, a.with_token);
    assert_eq!(delivery.secret.as_str(), TOKEN);
    let error = resolver.resolve_account("cs", "gone").err().unwrap();
    assert!(!error.contains(TOKEN), "{error}");
}
