use super::*;
use crate::docker_accounts::test_support::Installed;
use std::path::Path;

fn pinned(store: &Store, name: &str) -> Option<String> {
    super::pinned(store, name).unwrap()
}

fn store() -> Store {
    Store::open(Path::new(":memory:")).unwrap()
}

#[test]
fn a_pin_is_read_back_by_sandbox_name_and_cleared() {
    let store = store();
    assert_eq!(pinned(&store, "box-1"), None);
    pin(&store, "box-1", Some("acct-A")).unwrap();
    pin(&store, "box-2", Some("acct-B")).unwrap();
    assert_eq!(pinned(&store, "box-1").as_deref(), Some("acct-A"));
    assert_eq!(pinned(&store, "box-2").as_deref(), Some("acct-B"));
    let raw = store
        .setting("sandbox-docker-account:box-1")
        .unwrap()
        .unwrap();
    assert!(raw.contains("acct-A") && !raw.contains("dckr_pat"), "{raw}");
    clear_pin(&store, "box-1").unwrap();
    assert_eq!(pinned(&store, "box-1"), None);
    assert_eq!(pinned(&store, "box-2").as_deref(), Some("acct-B"));
}

#[test]
fn no_account_writes_no_pin_so_the_sandbox_keeps_the_machine_login() {
    let store = store();
    pin(&store, "box-1", None).unwrap();
    assert_eq!(store.setting(&pin_key("box-1")).unwrap(), None);
    assert_eq!(pinned(&store, "box-1"), None);
    // Clearing a pin that was never written writes nothing either.
    clear_pin(&store, "box-1").unwrap();
    assert_eq!(store.setting(&pin_key("box-1")).unwrap(), None);
}

#[test]
fn a_blank_pin_is_no_pin() {
    let store = store();
    store.save_setting(&pin_key("box-1"), "  ").unwrap();
    assert_eq!(pinned(&store, "box-1"), None);
}

#[test]
fn the_account_is_named_by_docker_id_and_never_by_token() {
    let installed = Installed::new();
    let id = installed.add("alice");
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("monocode.db")).unwrap();
    // This store's credentials database has no such account: a generic phrase.
    assert_eq!(describe(&store, &id), "its Docker account");
    let other = crate::docker_accounts::DockerAccounts::for_store(&store).unwrap();
    let added = other
        .add(
            "bob",
            concat!("dckr_pat_", "exampleTokenValue1234567890"),
            installed.hub.as_ref(),
        )
        .unwrap();
    let text = describe(&store, &added.id);
    assert_eq!(text, "Docker account bob");
    for jwt in installed.hub.issued() {
        assert!(!text.contains(&jwt));
    }
}

#[test]
fn pinning_no_account_clears_an_existing_pin() {
    let store = store();
    pin(&store, "box-1", Some("acct-A")).unwrap();
    assert_eq!(pinned(&store, "box-1").as_deref(), Some("acct-A"));
    pin(&store, "box-1", None).unwrap();
    assert_eq!(pinned(&store, "box-1"), None);
}

#[test]
fn a_plain_id_written_by_the_first_format_is_still_read() {
    let store = store();
    store.save_setting(&pin_key("box-1"), "acct-A").unwrap();
    assert_eq!(pinned(&store, "box-1").as_deref(), Some("acct-A"));
    // A pin of the new format with an unusable body can not be read: an error, never "no
    // account" (which would send the sandbox's calls under the machine login).
    store.save_setting(&pin_key("box-2"), "{broken").unwrap();
    assert!(super::pinned(&store, "box-2").is_err());
}

#[test]
fn the_pin_keeps_the_docker_id_and_a_failed_write_is_an_error() {
    let installed = Installed::new();
    let id = installed.add("alice");
    let store = store();
    pin(&store, "box-1", Some(&id)).unwrap();
    let raw = store.setting(&pin_key("box-1")).unwrap().unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["account"], id.as_str());
    assert_eq!(value["dockerId"], "alice");
    // A store that can not be written reports it (the start must not go on to create).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monocode.db");
    let file = Store::open(&path).unwrap();
    let side = rusqlite::Connection::open(&path).unwrap();
    side.execute_batch(
        "CREATE TRIGGER no_pins BEFORE INSERT ON native_settings
         WHEN NEW.key LIKE 'sandbox-docker-account:%' BEGIN SELECT RAISE(ABORT,'no'); END;",
    )
    .unwrap();
    let error = pin(&file, "box-1", Some(&id)).unwrap_err();
    assert!(error.contains("Docker account"), "{error}");
    assert_eq!(pinned(&file, "box-1"), None);
}

#[test]
fn a_pin_follows_the_docker_id_when_the_account_was_added_again() {
    let installed = Installed::new();
    let first = installed.add("alice");
    let store = store();
    pin(&store, "box-1", Some(&first)).unwrap();
    // While the account exists, it is the account.
    assert_eq!(pinned(&store, "box-1").as_deref(), Some(first.as_str()));
    // The owner removes the account and adds the same Docker ID again: a new id.
    installed.accounts.remove(&first).unwrap();
    assert_eq!(
        pinned(&store, "box-1").as_deref(),
        Some(first.as_str()),
        "no account with that Docker ID: the stale id stays, and fails with its own text"
    );
    let second = installed.add("alice");
    assert_ne!(first, second);
    assert_eq!(pinned(&store, "box-1").as_deref(), Some(second.as_str()));
    // Another Docker ID is never taken for it.
    let bob = installed.add("bob");
    assert_ne!(pinned(&store, "box-1").as_deref(), Some(bob.as_str()));
}

#[test]
fn a_pin_follows_the_docker_id_ignoring_case() {
    let installed = Installed::new();
    let first = installed.add("Alice");
    let store = store();
    pin(&store, "box-1", Some(&first)).unwrap();
    installed.accounts.remove(&first).unwrap();
    let second = installed.add("alice");
    assert_ne!(first, second);
    assert_eq!(pinned(&store, "box-1").as_deref(), Some(second.as_str()));
}

#[test]
fn an_unreadable_pin_is_an_error_never_no_account() {
    // The setting holds something that is not a pin.
    let store = store();
    store.save_setting(&pin_key("box-1"), "{not json").unwrap();
    let error = super::pinned(&store, "box-1").unwrap_err();
    assert!(error.contains("Could not read"), "{error}");
    // The setting table cannot be read at all.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monocode.db");
    let file = Store::open(&path).unwrap();
    pin(&file, "box-2", Some("acct-A")).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE native_settings;")
        .unwrap();
    assert!(super::pinned(&file, "box-2").is_err());
    assert!(clear_pin(&file, "box-2").is_err());
}

fn live_sandbox(store: &Store, name: &str) {
    store
        .remember_cloud_ref_project(
            "env",
            &serde_json::json!({"cwd": format!("/ws/{name}"), "id": name}),
            "/local/project",
            &crate::cloud_sessions::CloudRef::sandbox(name),
        )
        .unwrap();
}

#[test]
fn sandboxes_pinned_to_lists_only_live_sandboxes_of_that_account() {
    let store = store();
    pin(&store, "live-a", Some("acct-A")).unwrap();
    pin(&store, "live-b", Some("acct-B")).unwrap();
    pin(&store, "gone-a", Some("acct-A")).unwrap();
    live_sandbox(&store, "live-a");
    live_sandbox(&store, "live-b");
    assert_eq!(sandboxes_pinned_to(&store, "acct-A").unwrap(), ["live-a"]);
    assert_eq!(sandboxes_pinned_to(&store, "acct-B").unwrap(), ["live-b"]);
    assert!(sandboxes_pinned_to(&store, "nobody").unwrap().is_empty());
    // A cleared pin is no use.
    clear_pin(&store, "live-a").unwrap();
    assert!(sandboxes_pinned_to(&store, "acct-A").unwrap().is_empty());
}

#[test]
fn clearing_a_pin_deletes_the_setting_row() {
    let store = store();
    pin(&store, "box-1", Some("acct-A")).unwrap();
    clear_pin(&store, "box-1").unwrap();
    assert_eq!(store.setting(&pin_key("box-1")).unwrap(), None);
    assert!(store
        .settings_with_key_prefix(PIN_PREFIX)
        .unwrap()
        .is_empty());
}

#[test]
fn a_removed_account_names_the_sandbox_and_where_to_add_the_token_again() {
    let installed = Installed::new();
    let id = installed.add("alice");
    let store = store();
    pin(&store, "box-1", Some(&id)).unwrap();
    assert_eq!(removed_account_error(&store, "box-1"), None);
    installed.accounts.remove(&id).unwrap();
    let error = removed_account_error(&store, "box-1").unwrap();
    assert_eq!(
        error,
        "Sandbox box-1 was created with Docker account alice, which was removed. Add a token for alice again in Settings > Cloud."
    );
    assert!(!error.contains("Choose another"), "{error}");
    // Adding the same Docker ID again makes it usable again.
    installed.add("alice");
    assert_eq!(removed_account_error(&store, "box-1"), None);
    // A sandbox without a pin, or with a pin that has no recorded Docker ID, has no such text.
    assert_eq!(removed_account_error(&store, "box-2"), None);
    store.save_setting(&pin_key("box-3"), "old-id").unwrap();
    assert_eq!(removed_account_error(&store, "box-3"), None);
}
