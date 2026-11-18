//! Pure tests of `cloud.instances` (H-10, H-12): who sees which instance, the
//! count for the others, the cost and the stop report. Tests that need a shell
//! are in `shell/mobile_host/cloud_lifecycle_tests.rs`.
use super::instances::assemble;
use monocode_core::cloud_sessions::stopping::StopReport;
use monocode_core::cloud_sessions::workspaces::{
    CostEstimate, UsingSession, WorkspaceOverview, WorkspaceRow, WorkspaceState,
};
use monocode_core::cloud_sessions::{CloudBackendKind, CloudRef};
use serde_json::{Value, json};

const NOW: u64 = 10_000_000_000;

fn row(
    kind: CloudBackendKind,
    name: &str,
    state: WorkspaceState,
    sessions: &[i64],
) -> WorkspaceRow {
    WorkspaceRow {
        cloud: CloudRef {
            kind,
            name: name.into(),
        },
        id: None,
        account: None,
        account_label: None,
        display_name: format!("{name} display"),
        branch: Some("leak-branch".into()),
        size: "leak-size".into(),
        raw_state: format!("{state:?}"),
        state,
        created_ms: Some(NOW - 3_600_000),
        last_used_ms: Some(NOW - 60_000),
        ours: true,
        sessions: sessions
            .iter()
            .map(|id| UsingSession {
                id: *id,
                title: format!("title {id}"),
                busy: false,
                moved: false,
            })
            .collect(),
        unsaved: vec![],
        git: None,
        cost: CostEstimate {
            rate_per_hour: Some(0.36),
            ..CostEstimate::default()
        },
        has_web: true,
        stopping_since_ms: None,
        stop: None,
    }
}

fn overview(rows: Vec<WorkspaceRow>) -> WorkspaceOverview {
    WorkspaceOverview {
        project: "/work/p".into(),
        repo: Some("octo/widgets".into()),
        loaded_ms: NOW,
        rows,
        gh: None,
        codespaces_error: None,
        sandbox_problems: vec![],
        stop_unread: vec![],
    }
}

/// Sessions 1 and 2 are selected, 3 is not; every session id is its own catalog id.
fn uses(id: i64) -> Option<(String, bool, bool)> {
    (id != 99).then(|| (format!("c{id}"), id < 3, false))
}

fn names(reply: &Value) -> Vec<&str> {
    reply["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["instance"].as_str().unwrap())
        .collect()
}

fn rows() -> Vec<WorkspaceRow> {
    use CloudBackendKind::{Codespaces, DockerSandbox};
    vec![
        row(Codespaces, "mine", WorkspaceState::Running, &[1, 3]),
        row(Codespaces, "theirs", WorkspaceState::Running, &[3]),
        row(DockerSandbox, "failed", WorkspaceState::Failed, &[]),
        row(DockerSandbox, "orphan", WorkspaceState::Running, &[]),
        row(Codespaces, "idle-orphan", WorkspaceState::Stopped, &[]),
    ]
}

#[test]
fn an_all_projects_device_names_every_instance_including_failed_and_orphan_ones() {
    let reply = assemble(true, &[overview(rows())], NOW, &mut uses);
    assert_eq!(
        names(&reply),
        ["idle-orphan", "mine", "failed", "orphan", "theirs"]
    );
    assert!(reply.get("otherCount").is_none());
    let mine = reply["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["instance"] == "mine")
        .unwrap();
    // Every session of the instance, by catalog id.
    assert_eq!(
        mine["sessions"],
        json!([
            {"sessionId": "c1", "title": "title 1", "busy": false},
            {"sessionId": "c3", "title": "title 3", "busy": false},
        ])
    );
}

#[test]
fn a_device_with_a_selection_sees_only_its_sessions_and_a_count_of_the_billing_rest() {
    let reply = assemble(false, &[overview(rows())], NOW, &mut uses);
    // "theirs" is used by an unselected project only; it is neither named nor counted.
    assert_eq!(names(&reply), ["mine"]);
    // The failed and the running orphan bill and are not its own: a count, no names.
    assert_eq!(reply["otherCount"], 2);
    let mine = &reply["instances"][0];
    assert_eq!(
        mine["sessions"],
        json!([{"sessionId": "c1", "title": "title 1", "busy": false}]),
        "the title of an unselected project's session is not told"
    );
    let text = reply.to_string();
    for hidden in ["failed", "orphan", "theirs", "title 3", "c3"] {
        assert!(!text.contains(hidden), "{hidden} in {text}");
    }
}

#[test]
fn a_session_the_desktop_does_not_know_is_never_shown_to_a_device_with_a_selection() {
    let one = overview(vec![row(
        CloudBackendKind::Codespaces,
        "mine",
        WorkspaceState::Running,
        &[99],
    )]);
    let reply = assemble(false, &[one], NOW, &mut uses);
    assert_eq!(names(&reply), Vec::<&str>::new());
}

#[test]
fn rows_carry_state_cost_last_use_and_the_stop_report_and_no_extra_data() {
    use CloudBackendKind::Codespaces;
    let mut stopping = row(Codespaces, "a", WorkspaceState::Stopping, &[1]);
    stopping.stop = Some(StopReport::Stopping);
    let mut slow = row(Codespaces, "b", WorkspaceState::Hibernating, &[1]);
    slow.stop = Some(StopReport::TakingLong);
    let mut again = row(Codespaces, "c", WorkspaceState::Running, &[1]);
    again.stop = Some(StopReport::StillRunning(
        "leak-reason with a /leak/path".into(),
    ));
    let mut done = row(Codespaces, "d", WorkspaceState::Stopped, &[1]);
    done.stop = Some(StopReport::Stopped);
    let mut free = row(Codespaces, "e", WorkspaceState::Unknown, &[1]);
    free.cost = CostEstimate::default();
    let reply = assemble(
        true,
        &[overview(vec![stopping, slow, again, done, free])],
        NOW,
        &mut uses,
    );
    let by = |name: &str| {
        reply["instances"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["instance"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(by("a")["stopReport"], "stopping");
    assert_eq!(by("a")["state"], "stopping");
    assert_eq!(by("b")["stopReport"], "takingLong");
    assert_eq!(by("b")["state"], "stopping");
    assert_eq!(by("c")["stopReport"], "stillRunning");
    assert_eq!(by("c")["state"], "available");
    assert!(
        by("d").get("stopReport").is_none(),
        "a settled stop reports nothing"
    );
    assert_eq!(by("d")["state"], "stopped");
    assert_eq!(by("e")["state"], "unknown");
    assert!(by("e").get("costEstimatePerHour").is_none());
    assert_eq!(by("a")["costEstimatePerHour"], 0.36);
    assert_eq!(by("a")["lastUsedAt"], NOW - 60_000);
    assert_eq!(by("a")["kind"], "codespace");
    assert_eq!(by("a")["displayName"], "a display");
    let text = reply.to_string();
    for needle in ["leak-", "/leak", "ssh", "token"] {
        assert!(!text.contains(needle), "{needle} in {text}");
    }
}

#[test]
fn an_instance_listed_by_two_projects_is_one_row_with_every_session() {
    use CloudBackendKind::Codespaces;
    let first = overview(vec![row(
        Codespaces,
        "shared",
        WorkspaceState::Running,
        &[1],
    )]);
    let second = overview(vec![row(
        Codespaces,
        "shared",
        WorkspaceState::Running,
        &[1, 2],
    )]);
    let reply = assemble(true, &[first, second], NOW, &mut uses);
    assert_eq!(names(&reply), ["shared"]);
    assert_eq!(
        reply["instances"][0]["sessions"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn a_busy_session_is_busy_whichever_side_knows_it() {
    use CloudBackendKind::Codespaces;
    let one = overview(vec![row(Codespaces, "x", WorkspaceState::Running, &[1])]);
    let reply = assemble(true, &[one], NOW, &mut |id| {
        Some((format!("c{id}"), true, true))
    });
    assert_eq!(reply["instances"][0]["sessions"][0]["busy"], true);
}
