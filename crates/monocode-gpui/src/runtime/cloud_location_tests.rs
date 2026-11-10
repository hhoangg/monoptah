//! The local runtime never starts a turn for a session that lives in the cloud (design 4.9 R2).
use super::*;
use monocode_core::cloud_sessions::CloudRef;
use monocode_core::store::{CloudLocation, SessionLocation};

/// A local session with 10 rows written before the move, location Cloud, `moved_at_row` 10.
fn moved_session(store: &mut Store) -> Session {
    let session = store.create("claude", "/Users/me/repo").unwrap();
    let rows: Vec<Row> = (0..10)
        .map(|i| Row {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            text: format!("local {i}"),
            first: true,
            sent_at: None,
        })
        .collect();
    store.write_rows(session.id, 0, &rows).unwrap();
    store
        .set_cloud_location(
            session.id,
            &SessionLocation::Cloud(CloudLocation {
                cloud: CloudRef::codespace("octo-abc"),
                environment_id: "env".into(),
                project_id: "project".into(),
                host_session_id: "host-session".into(),
                remote_cwd: "/host/repo".into(),
                local_cwd: "/Users/me/repo".into(),
                moved_at_row: 10,
                pushed_branch: None,
            }),
        )
        .unwrap();
    session
}

#[test]
fn the_local_runtime_refuses_a_cloud_session_before_it_starts_a_turn() {
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let summary = moved_session(&mut store);
    let store: Database = Arc::new(Mutex::new(store));
    let (tx, rx) = async_channel::unbounded();
    let (_controls_tx, controls) = std::sync::mpsc::channel();
    run_turn_using(
        TurnInput {
            summary: summary.clone(),
            prompt: "hello".into(),
            settings: Settings::default(),
        },
        controls,
        store.clone(),
        Arc::new(AtomicBool::new(false)),
        tx,
        // Resolving a provider binary means a turn is starting.
        |_| panic!("the local provider must not be resolved for a cloud session"),
    );
    let update = rx.try_recv().expect("one refusal");
    assert!(update.finished);
    assert_eq!(update.status, monocode_core::store::LOCAL_RUN_REFUSED);
    assert!(rx.try_recv().is_err(), "nothing else is sent");
    // Nothing was written: the 10 local rows are untouched and no turn was configured.
    let guard = locked(&store).unwrap();
    assert_eq!(guard.load(summary.id).unwrap().rows.len(), 10);
    assert!(guard.agent_settings(summary.id).unwrap().is_none());
}

#[test]
fn the_local_runtime_still_starts_once_the_session_is_local_again() {
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let summary = moved_session(&mut store);
    store.reset_cloud_location_to_local(summary.id).unwrap();
    assert!(store.require_local_run(summary.id).is_ok());
}
