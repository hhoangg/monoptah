use super::*;
use crate::docker_accounts::test_support::Installed;
use std::path::Path;

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
    // And a pin of the new format with an unusable body is no pin.
    store.save_setting(&pin_key("box-2"), "{broken").unwrap();
    assert_eq!(pinned(&store, "box-2"), None);
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
