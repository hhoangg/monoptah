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
    pin(&store, "box-1", Some("acct-A"));
    pin(&store, "box-2", Some("acct-B"));
    assert_eq!(pinned(&store, "box-1").as_deref(), Some("acct-A"));
    assert_eq!(pinned(&store, "box-2").as_deref(), Some("acct-B"));
    assert_eq!(
        store
            .setting("sandbox-docker-account:box-1")
            .unwrap()
            .as_deref(),
        Some("acct-A")
    );
    clear_pin(&store, "box-1");
    assert_eq!(pinned(&store, "box-1"), None);
    assert_eq!(pinned(&store, "box-2").as_deref(), Some("acct-B"));
}

#[test]
fn no_account_writes_no_pin_so_the_sandbox_keeps_the_machine_login() {
    let store = store();
    pin(&store, "box-1", None);
    assert_eq!(store.setting(&pin_key("box-1")).unwrap(), None);
    assert_eq!(pinned(&store, "box-1"), None);
    // Clearing a pin that was never written writes nothing either.
    clear_pin(&store, "box-1");
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
