use super::*;
use crate::store::cloud_rows::NOT_CLOUD;
use crate::store::location::tests::cloud_location;
use crate::store::location::WORKSPACE_LOST;
use crate::store::SessionLocation;
use serde_json::Value;
use std::path::Path;

fn snapshot(revision: u64, blocks: &[(&str, &str)]) -> Value {
    let blocks: Vec<Value> = blocks
        .iter()
        .enumerate()
        .map(|(i, (role, text))| json!({"id": format!("b{i}"), "role": role, "text": text}))
        .collect();
    json!({"revision": revision, "status": "idle", "session": {"id": "host-1", "blocks": blocks}})
}

fn local_rows(count: usize) -> Vec<Row> {
    (0..count)
        .map(|i| Row {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            text: format!("local {i}"),
            first: true,
            sent_at: Some(i as u64 + 1),
        })
        .collect()
}

/// A real file store with `count` local rows, then moved to the cloud at that row.
fn moved(dir: &Path, count: usize) -> (Store, i64) {
    let mut store = Store::open_monocode(&dir.join("monocode.db")).unwrap();
    let id = store.create("claude", "/Users/me/repo").unwrap().id;
    store.write_rows(id, 0, &local_rows(count)).unwrap();
    set_cloud(&store, id, count);
    (store, id)
}

fn set_cloud(store: &Store, id: i64, moved_at_row: usize) {
    let mut location = cloud_location();
    location.moved_at_row = moved_at_row;
    store
        .set_cloud_location(id, &SessionLocation::Cloud(location))
        .unwrap();
}

fn texts(store: &Store, id: i64) -> Vec<String> {
    blocks::project(&store.shared_blocks(id).unwrap())
        .unwrap()
        .into_iter()
        .map(|r| format!("{}:{}", r.role, r.text))
        .collect()
}

fn raw_blocks(store: &Store, id: i64) -> String {
    let original = store.persisted_session_id(id).unwrap();
    store
        .conn
        .query_row(
            "SELECT blocks_json FROM sessions WHERE id=?1",
            [original],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn the_anchor_equals_the_native_anchor_and_leaves_the_conflict_snapshot_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut shared = Store::open_monocode(&dir.path().join("monocode.db")).unwrap();
    let mut native = Store::open(Path::new(":memory:")).unwrap();
    let rows = local_rows(5);
    let a = shared.create("claude", "/p").unwrap().id;
    let b = native.create("claude", "/p").unwrap().id;
    shared.write_rows(a, 0, &rows).unwrap();
    native.write_rows(b, 0, &rows).unwrap();
    let anchor = shared.row_anchor(a).unwrap();
    assert_eq!(anchor, native.row_anchor(b).unwrap());
    assert_eq!(anchor.count, 5);
    assert!(anchor.last.is_some());
    // An empty session has the same anchor too.
    let empty = shared.create("claude", "/p").unwrap().id;
    assert_eq!(shared.row_anchor(empty).unwrap(), RowAnchor::default());

    // This window loaded the history, then another window changed it. Reading the anchor must
    // not refresh the conflict snapshot, so the next save still reports the change.
    shared.load(a).unwrap();
    let original = shared.persisted_session_id(a).unwrap();
    let mut blocks = shared.shared_blocks(a).unwrap();
    blocks[0]["text"] = json!("edited elsewhere");
    shared
        .conn
        .execute(
            "UPDATE sessions SET blocks_json=?2 WHERE id=?1",
            rusqlite::params![original, blocks_json(&blocks)],
        )
        .unwrap();
    shared.row_anchor(a).unwrap();
    let error = shared.write_rows(a, 5, &rows[..1]).unwrap_err();
    assert!(error.contains("changed in another window"), "{error}");
}

fn blocks_json(blocks: &[Value]) -> String {
    serde_json::to_string(blocks).unwrap()
}

#[test]
fn a_moved_session_appends_cloud_turns_after_the_local_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 4);
    let before = store.shared_blocks(id).unwrap();
    let before_rows = store.shared_load(id).unwrap().rows;
    let first = snapshot(1, &[("user", "cloud q"), ("assistant", "cloud a")]);
    let out = store.append_remote_rows(id, &first).unwrap();
    assert_eq!((out.start, out.written, out.truncated), (4, 2, 0));
    assert_eq!(
        store.cloud_synced_revision("env", "host-1").unwrap(),
        Some(1)
    );
    let loaded = store.load(id).unwrap().rows;
    assert_eq!(&loaded[..4], &before_rows[..]);
    assert_eq!(loaded.len(), 6);
    assert_eq!(
        &texts(&store, id)[4..],
        ["user:cloud q", "assistant:cloud a"]
    );
    assert_eq!(
        &store.shared_blocks(id).unwrap()[..before.len()],
        &before[..]
    );

    // The same snapshot again writes nothing.
    let stored = raw_blocks(&store, id);
    let again = store.append_remote_rows(id, &first).unwrap();
    assert_eq!((again.written, again.truncated), (0, 0));
    assert_eq!(raw_blocks(&store, id), stored);
}

#[test]
fn streaming_growth_and_truncation_only_rewrite_the_cloud_tail() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 4);
    let before = store.shared_blocks(id).unwrap();
    let mut seen = Vec::new();
    for (revision, host) in [
        (1, vec![("user", "q"), ("assistant", "par")]),
        (2, vec![("user", "q"), ("assistant", "partial answer")]),
        (
            3,
            vec![
                ("user", "q"),
                ("assistant", "partial answer"),
                ("user", "more"),
            ],
        ),
        (4, vec![("user", "q")]),
        (5, vec![]),
    ] {
        store
            .append_remote_rows(id, &snapshot(revision, &host))
            .unwrap();
        let expected: Vec<String> = (0..4)
            .map(|i| {
                format!(
                    "{}:local {i}",
                    if i % 2 == 0 { "user" } else { "assistant" }
                )
            })
            .chain(host.iter().map(|(r, t)| format!("{r}:{t}")))
            .collect();
        assert_eq!(texts(&store, id), expected, "revision {revision}");
        assert_eq!(
            &store.shared_blocks(id).unwrap()[..before.len()],
            &before[..]
        );
        seen.push(raw_blocks(&store, id));
    }
    // Only the first four blocks are common to all of them, byte for byte.
    let head = serde_json::to_string(&before).unwrap();
    for raw in &seen {
        assert!(raw.starts_with(head.trim_end_matches(']')), "{raw}");
    }
    let out = store
        .append_remote_rows(id, &snapshot(6, &[("user", "again")]))
        .unwrap();
    assert_eq!((out.start, out.written, out.truncated), (4, 1, 0));
}

#[test]
fn a_shorter_host_history_reports_the_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 2);
    store
        .append_remote_rows(
            id,
            &snapshot(1, &[("user", "a"), ("assistant", "b"), ("user", "c")]),
        )
        .unwrap();
    let out = store
        .append_remote_rows(id, &snapshot(2, &[("user", "a")]))
        .unwrap();
    assert_eq!((out.start, out.written, out.truncated), (3, 0, 2));
    assert_eq!(texts(&store, id).len(), 3);
}

#[test]
fn the_first_sync_with_a_different_row_count_is_diverged_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_monocode(&dir.path().join("monocode.db")).unwrap();
    let id = store.create("claude", "/p").unwrap().id;
    store.write_rows(id, 0, &local_rows(5)).unwrap();
    set_cloud(&store, id, 4);
    let stored = raw_blocks(&store, id);
    let error = store
        .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
        .unwrap_err();
    assert_eq!(error, DIVERGED);
    assert_eq!(raw_blocks(&store, id), stored);
    assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
    // Fewer rows than the move point is refused after a sync too.
    set_cloud(&store, id, 9);
    store.save_setting("cloud-synced:env:host-1", "1").unwrap();
    assert_eq!(
        store
            .append_remote_rows(id, &snapshot(2, &[("user", "q")]))
            .unwrap_err(),
        DIVERGED
    );
    assert_eq!(raw_blocks(&store, id), stored);
}

#[test]
fn another_cloud_session_a_local_session_and_a_lost_workspace_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 2);
    let stored = raw_blocks(&store, id);
    let rows = [Row {
        role: "user".into(),
        text: "q".into(),
        first: true,
        sent_at: None,
    }];
    assert_eq!(
        store
            .append_projected_rows(id, "other-env", "host-1", 1, &rows)
            .unwrap_err(),
        NOT_CLOUD
    );
    assert_eq!(
        store
            .append_projected_rows(id, "env", "another-host", 1, &rows)
            .unwrap_err(),
        NOT_CLOUD
    );
    store.record_cloud_sync_error(id, WORKSPACE_LOST).unwrap();
    assert_eq!(
        store
            .append_projected_rows(id, "env", "host-1", 1, &rows)
            .unwrap_err(),
        WORKSPACE_LOST
    );
    assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
    store.reset_cloud_location_to_local(id).unwrap();
    assert_eq!(
        store
            .append_projected_rows(id, "env", "host-1", 1, &rows)
            .unwrap_err(),
        NOT_CLOUD
    );
    assert!(store
        .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
        .is_err());
    assert_eq!(raw_blocks(&store, id), stored);
}

#[test]
fn a_successful_append_clears_the_sync_error() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 2);
    store.record_cloud_sync_error(id, "boom").unwrap();
    assert_eq!(store.cloud_sync_error(id).unwrap().as_deref(), Some("boom"));
    store
        .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
        .unwrap();
    assert_eq!(store.cloud_sync_error(id).unwrap(), None);
}

#[test]
fn a_reopened_store_sees_the_same_rows_and_a_repeat_sync_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 4);
    let snap = snapshot(2, &[("user", "q"), ("assistant", "a")]);
    store.append_remote_rows(id, &snap).unwrap();
    let anchor = store.row_anchor(id).unwrap();
    let stored = raw_blocks(&store, id);
    drop(store);
    let mut store = Store::open_monocode(&dir.path().join("monocode.db")).unwrap();
    assert_eq!(store.row_anchor(id).unwrap(), anchor);
    assert_eq!(anchor.count, 6);
    assert_eq!(
        store.cloud_synced_revision("env", "host-1").unwrap(),
        Some(2)
    );
    let out = store.append_remote_rows(id, &snap).unwrap();
    assert_eq!((out.written, out.truncated), (0, 0));
    assert_eq!(raw_blocks(&store, id), stored);
}

#[test]
fn a_stale_conflict_snapshot_stays_stale_and_a_current_one_follows_the_write() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 2);
    // No snapshot yet: the append does not invent one.
    store
        .shared
        .as_ref()
        .unwrap()
        .snapshots
        .borrow_mut()
        .remove(&id);
    store
        .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
        .unwrap();
    assert!(!store
        .shared
        .as_ref()
        .unwrap()
        .snapshots
        .borrow()
        .contains_key(&id));
    // A matching snapshot moves along with the write.
    store.load(id).unwrap();
    store
        .append_remote_rows(id, &snapshot(2, &[("user", "q"), ("assistant", "a")]))
        .unwrap();
    let current = store.shared_blocks(id).unwrap();
    assert!(store.shared.as_ref().unwrap().snapshots.borrow()[&id].matches(&current));
    // A stale one is left stale.
    let stale = *store
        .shared
        .as_ref()
        .unwrap()
        .snapshots
        .borrow()
        .get(&id)
        .unwrap();
    let original = store.persisted_session_id(id).unwrap();
    let mut changed = current.clone();
    changed[0]["text"] = json!("edited elsewhere");
    store
        .conn
        .execute(
            "UPDATE sessions SET blocks_json=?2 WHERE id=?1",
            rusqlite::params![original, blocks_json(&changed)],
        )
        .unwrap();
    store
        .append_remote_rows(id, &snapshot(3, &[("user", "q"), ("assistant", "b")]))
        .unwrap();
    assert_eq!(
        store.shared.as_ref().unwrap().snapshots.borrow()[&id],
        stale
    );
}

#[test]
fn an_indexed_conversation_is_refused_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 2);
    let original = store.persisted_session_id(id).unwrap();
    store
        .conn
        .execute_batch("CREATE TABLE mono_transcripts (session_id TEXT PRIMARY KEY)")
        .unwrap();
    store
        .conn
        .execute("INSERT INTO mono_transcripts VALUES(?1)", [&original])
        .unwrap();
    let stored = raw_blocks(&store, id);
    assert_eq!(store.row_anchor(id).unwrap_err(), INDEXED_UNSUPPORTED);
    assert_eq!(
        store
            .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
            .unwrap_err(),
        INDEXED_UNSUPPORTED
    );
    assert_eq!(raw_blocks(&store, id), stored);
    assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
}

fn qualified(revision: u64, blocks: &[Value]) -> Value {
    let mut value = json!({"revision": revision, "status": "idle",
        "session": {"id": "host-1", "blocks": blocks}});
    crate::remote::attachments::qualify_snapshot(&mut value, "env", "host-1");
    value
}

fn updated_at(store: &Store, id: i64) -> i64 {
    let original = store.persisted_session_id(id).unwrap();
    store
        .conn
        .query_row(
            "SELECT updated_at FROM sessions WHERE id=?1",
            [original],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn a_qualified_snapshot_syncs_like_the_native_store_and_then_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut shared, id) = moved(dir.path(), 2);
    let mut native = Store::open(Path::new(":memory:")).unwrap();
    let nid = native.create("claude", "/Users/me/repo").unwrap().id;
    native.write_rows(nid, 0, &local_rows(2)).unwrap();
    set_cloud(&native, nid, 2);
    let snap = qualified(
        4,
        &[
            json!({"id":"h1","role":"user","text":"first"}),
            json!({"id":"h2","role":"assistant","text":"answer"}),
            json!({"id":"h3","role":"user","text":"second"}),
        ],
    );
    let mut stored = None;
    for round in 0..3 {
        let a = shared.append_remote_rows(id, &snap).unwrap();
        let b = native.append_remote_rows(nid, &snap).unwrap();
        assert_eq!(a, b, "round {round}");
        let state = (raw_blocks(&shared, id), updated_at(&shared, id));
        match &stored {
            None => stored = Some(state),
            Some(first) => assert_eq!(&state, first, "round {round}"),
        }
    }
    let rows = shared.load(id).unwrap().rows;
    assert_eq!(rows.len(), 2 + 5);
}

#[test]
fn a_host_plan_round_trips_and_a_repeat_sync_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 2);
    let snap = qualified(
        1,
        &[
            json!({"id":"h1","role":"user","text":"plan it"}),
            json!({"id":"plan-1","role":"plan","text":"1. do it","plan":{"status":"ready"}}),
        ],
    );
    let first = store.append_remote_rows(id, &snap).unwrap();
    assert_eq!((first.start, first.written), (2, 3));
    let rows = store.load(id).unwrap().rows;
    assert!(rows.iter().any(|r| r.role == "remote-plan"), "{rows:?}");
    let stored = (raw_blocks(&store, id), updated_at(&store, id));
    let again = store.append_remote_rows(id, &snap).unwrap();
    assert_eq!((again.written, again.truncated), (0, 0));
    assert_eq!((raw_blocks(&store, id), updated_at(&store, id)), stored);
}

#[test]
fn a_row_kind_that_cannot_be_stored_is_refused_without_a_cursor_or_a_write() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, id) = moved(dir.path(), 2);
    let stored = raw_blocks(&store, id);
    let rows = [Row {
        role: "mono-launch".into(),
        text: "not json".into(),
        first: true,
        sent_at: None,
    }];
    let error = store
        .append_projected_rows(id, "env", "host-1", 1, &rows)
        .unwrap_err();
    assert_eq!(error, CANNOT_STORE);
    assert_eq!(raw_blocks(&store, id), stored);
    assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
}

#[test]
fn a_terminal_session_is_refused_by_both_functions_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_monocode(&dir.path().join("monocode.db")).unwrap();
    let id = store
        .create_with_surface("claude", "/p", crate::surface::Surface::Tui)
        .unwrap()
        .id;
    set_cloud(&store, id, 0);
    let stored = raw_blocks(&store, id);
    assert_eq!(store.row_anchor(id).unwrap_err(), TERMINAL_UNSUPPORTED);
    assert_eq!(
        store
            .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
            .unwrap_err(),
        TERMINAL_UNSUPPORTED
    );
    assert_eq!(raw_blocks(&store, id), stored);
    assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
}

fn task_snapshot(revision: u64, items: &[&str], explanation: Option<&str>) -> Value {
    let items: Vec<Value> = items
        .iter()
        .map(|text| json!({"content": text, "status": "pending"}))
        .collect();
    let mut list = json!({"key":"todo","providerSessionId":"p1","items":items});
    if let Some(text) = explanation {
        list["explanation"] = json!(text);
    }
    let tasks = json!({"id":"t1","role":"tasks","text":"","taskList":list});
    qualified(
        revision,
        &[json!({"id":"h1","role":"user","text":"do it"}), tasks],
    )
}

fn assert_syncs_like_native(snapshots: &[Value]) -> (Store, i64) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (mut shared, id) = moved(dir.path(), 2);
    let mut native = Store::open(Path::new(":memory:")).unwrap();
    let nid = native.create("claude", "/Users/me/repo").unwrap().id;
    native.write_rows(nid, 0, &local_rows(2)).unwrap();
    set_cloud(&native, nid, 2);
    let mut previous: Option<(Value, (String, i64))> = None;
    for snap in snapshots {
        let a = shared.append_remote_rows(id, snap).unwrap();
        let b = native.append_remote_rows(nid, snap).unwrap();
        assert_eq!(a, b, "{snap}");
        let state = (raw_blocks(&shared, id), updated_at(&shared, id));
        if let Some((last, before)) = &previous {
            if last == snap {
                assert_eq!(&state, before, "a repeat sync must write nothing");
                assert_eq!((a.written, a.truncated), (0, 0));
            }
        }
        previous = Some((snap.clone(), state));
    }
    (shared, id)
}

#[test]
fn a_task_list_syncs_stably_and_updates_in_place() {
    let plain = task_snapshot(1, &["a", "b"], None);
    let explained = task_snapshot(2, &["a", "b"], Some("why"));
    let changed = task_snapshot(3, &["a", "b", "c"], Some("why"));
    let (store, id) = assert_syncs_like_native(&[
        plain.clone(),
        plain.clone(),
        plain,
        explained.clone(),
        explained,
        changed.clone(),
        changed,
    ]);
    let rows = store.load(id).unwrap().rows;
    assert!(rows.last().unwrap().text.contains("\"c\""), "{rows:?}");
}

#[test]
fn an_agent_tool_without_an_agent_run_and_a_plan_without_an_id_sync_stably() {
    let agent = qualified(
        1,
        &[
            json!({"id":"h1","role":"user","text":"go"}),
            json!({"id":"a1","role":"tool","text":"Task: look around","tool":{"kind":"agent","title":"Task: look around"},"status":"completed"}),
            json!({"role":"plan","text":"1. step","plan":{"status":"ready"}}),
        ],
    );
    assert_syncs_like_native(&[agent.clone(), agent.clone(), agent]);
}
