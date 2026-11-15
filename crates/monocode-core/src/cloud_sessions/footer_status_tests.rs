//! The footer chip's reading of an overview: when it shows, and when it warns. Rows are built by
//! hand with obviously fake names; nothing here runs gh or sbx.
use super::footer_status::*;
use super::stopping::{StopReport, STOPPING_WARN};
use super::workspaces::{
    CostEstimate, UsingSession, WorkspaceOverview, WorkspaceRow, WorkspaceState,
};
use super::{CloudBackendKind, CloudRef};

const NOW: u64 = 1_800_000_000_000;
const HOUR_MS: u64 = 3_600_000;

fn row(name: &str, state: WorkspaceState, hours_old: u64) -> WorkspaceRow {
    WorkspaceRow {
        cloud: CloudRef::codespace(name),
        id: None,
        account: None,
        account_label: None,
        display_name: name.into(),
        branch: None,
        size: String::new(),
        raw_state: String::new(),
        state,
        created_ms: Some(NOW - hours_old * HOUR_MS),
        last_used_ms: None,
        ours: true,
        sessions: vec![session(false)],
        unsaved: vec![],
        git: None,
        cost: CostEstimate::default(),
        has_web: false,
        stopping_since_ms: None,
        stop: None,
    }
}

fn session(busy: bool) -> UsingSession {
    UsingSession {
        id: 1,
        title: "fake".into(),
        busy,
        moved: false,
    }
}

fn view(rows: Vec<WorkspaceRow>) -> WorkspaceOverview {
    WorkspaceOverview {
        project: "/work/fake".into(),
        repo: None,
        loaded_ms: NOW,
        rows,
        gh: None,
        codespaces_error: None,
        sandbox_problems: vec![],
        stop_unread: vec![],
    }
}

fn status(rows: Vec<WorkspaceRow>) -> FooterStatus {
    FooterStatus::of(&view(rows), NOW)
}

#[test]
fn nothing_running_or_stopping_hides_the_chip_whatever_else_is_listed() {
    let none = status(vec![]);
    assert!(!none.visible() && !none.warns());
    // Stopped, failed and unknown workspaces are neither running nor stopping: the chip hides.
    let idle = status(vec![
        row("a", WorkspaceState::Stopped, 1),
        row("c", WorkspaceState::Failed, 1),
        row("d", WorkspaceState::Unknown, 1),
    ]);
    assert_eq!((idle.running, idle.stopping), (0, 0));
    assert!(!idle.visible());
}

/// A workspace that has been stopping since `minutes` before [`NOW`].
fn stopping_for(name: &str, state: WorkspaceState, minutes: u64) -> WorkspaceRow {
    let mut r = row(name, state, 1);
    r.stopping_since_ms = Some(NOW - minutes * 60_000);
    r
}

#[test]
fn a_stopping_workspace_keeps_the_chip_on_screen_in_the_neutral_colour() {
    for state in [WorkspaceState::Stopping, WorkspaceState::Hibernating] {
        let s = status(vec![stopping_for("down", state, 2)]);
        assert!(
            s.visible(),
            "{state:?}: the chip must not vanish while it goes down"
        );
        assert_eq!((s.running, s.stopping), (0, 1));
        assert!(!s.warns(), "{state:?}: two minutes is normal");
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].state, state);
        assert_eq!(s.entries[0].concern, None);
    }
    // Running and stopping side by side are counted apart.
    let s = status(vec![
        row("up", WorkspaceState::Running, 1),
        stopping_for("down", WorkspaceState::Stopping, 1),
    ]);
    assert_eq!((s.running, s.stopping), (1, 1));
    assert_eq!(s.entries.len(), 2);
    // Only when it has stopped does the chip go.
    let s = status(vec![row("down", WorkspaceState::Stopped, 1)]);
    assert!(!s.visible());
}

#[test]
fn a_workspace_stopping_for_more_than_fifteen_minutes_warns() {
    assert_eq!(
        STOPPING_WARN.as_secs(),
        15 * 60,
        "documented in cloud-workspaces.md"
    );
    let under = status(vec![stopping_for("down", WorkspaceState::Stopping, 14)]);
    assert!(under.visible() && !under.warns());
    let at = status(vec![stopping_for("down", WorkspaceState::Stopping, 15)]);
    assert!(at.visible() && at.warns());
    assert_eq!(at.entries[0].concern, Some(Concern::StuckStopping));
    let hibernating = status(vec![stopping_for("down", WorkspaceState::Hibernating, 40)]);
    assert!(hibernating.warns());
    // The age follows the clock, not the moment the view was loaded.
    let aging = view(vec![stopping_for("down", WorkspaceState::Stopping, 14)]);
    assert!(!FooterStatus::of(&aging, NOW).warns());
    assert!(FooterStatus::of(&aging, NOW + 60_000).warns());
    // Since when is unknown: it cannot be called stuck.
    let mut unknown = stopping_for("down", WorkspaceState::Stopping, 90);
    unknown.stopping_since_ms = None;
    assert!(!status(vec![unknown]).warns());
}

#[test]
fn a_workspace_that_is_running_again_after_a_stop_warns() {
    let mut r = row("back", WorkspaceState::Running, 1);
    r.stop = Some(StopReport::StillRunning("fake reason".into()));
    let s = status(vec![r]);
    assert!(s.visible() && s.warns());
    assert_eq!(s.entries[0].concern, Some(Concern::StopUndone));
}

#[test]
fn running_and_starting_workspaces_are_counted_and_named() {
    let mut sandbox = row("sbx", WorkspaceState::Starting, 1);
    sandbox.cloud = CloudRef::sandbox("sbx");
    let s = status(vec![
        row("a", WorkspaceState::Running, 1),
        sandbox,
        row("b", WorkspaceState::Stopped, 1),
    ]);
    assert!(s.visible());
    assert_eq!(s.running, 2);
    let names: Vec<_> = s.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["a", "sbx"], "a stopped workspace is not named");
    assert_eq!(s.entries[1].backend, CloudBackendKind::DockerSandbox);
    assert!(!s.warns(), "a used, young workspace is normal");
}

#[test]
fn an_orphan_that_is_up_warns_but_a_stopped_one_or_a_foreign_one_does_not() {
    let mut orphan = row("lost", WorkspaceState::Running, 1);
    orphan.sessions.clear();
    let s = status(vec![orphan.clone()]);
    assert!(s.visible() && s.warns());
    assert_eq!(s.entries[0].concern, Some(Concern::Orphan));

    // Its sessions went but it is stopping: it is named, and nothing more can be done than wait,
    // so it is neutral (it warns only once the stop has taken too long).
    let mut stopping = orphan.clone();
    stopping.state = WorkspaceState::Stopping;
    stopping.stopping_since_ms = Some(NOW - 60_000);
    let s = status(vec![row("up", WorkspaceState::Running, 1), stopping]);
    assert_eq!((s.running, s.stopping), (1, 1));
    assert_eq!(s.entries[1].concern, None);
    assert!(!s.warns());

    // Stopped: no cost, no flag, not named.
    let mut stopped = orphan.clone();
    stopped.state = WorkspaceState::Stopped;
    let s = status(vec![row("up", WorkspaceState::Running, 1), stopped]);
    assert!(!s.warns());
    assert_eq!(s.entries.len(), 1);

    // Not this app's: another app or a person may be using it, so it is never an orphan.
    let mut foreign = orphan;
    foreign.ours = false;
    let s = status(vec![foreign]);
    assert!(s.visible() && !s.warns());
}

#[test]
fn a_failed_workspace_warns_once_the_chip_is_visible() {
    let s = status(vec![
        row("up", WorkspaceState::Running, 1),
        row("broken", WorkspaceState::Failed, 1),
    ]);
    assert!(s.visible() && s.warns());
    assert_eq!(s.running, 1, "a failed workspace is not counted as running");
    assert_eq!(s.entries[1].name, "broken");
    assert_eq!(s.entries[1].concern, Some(Concern::Failed));
}

#[test]
fn running_past_the_threshold_with_no_busy_session_warns() {
    let hours = LONG_RUNNING.as_secs() / 3600;
    assert_eq!(hours, 6, "documented in cloud-workspaces.md");
    let just_under = status(vec![row("a", WorkspaceState::Running, hours - 1)]);
    assert!(!just_under.warns());
    let at = status(vec![row("a", WorkspaceState::Running, hours)]);
    assert_eq!(at.entries[0].concern, Some(Concern::LongRunning));

    // An idle session is still "no busy session"; a working one clears it.
    let mut busy = row("a", WorkspaceState::Running, hours + 5);
    busy.sessions = vec![session(true)];
    assert!(!status(vec![busy]).warns());

    // A workspace whose age is unknown cannot be called long-running.
    let mut unknown = row("a", WorkspaceState::Running, hours + 5);
    unknown.created_ms = None;
    assert!(!status(vec![unknown]).warns());

    // The age follows the clock, not the moment the view was loaded.
    let aging = view(vec![row("a", WorkspaceState::Running, hours - 1)]);
    assert!(!FooterStatus::of(&aging, NOW).warns());
    assert!(FooterStatus::of(&aging, NOW + HOUR_MS).warns());
}

#[test]
fn the_most_serious_concern_wins() {
    let mut r = row("a", WorkspaceState::Failed, 100);
    r.sessions.clear();
    assert_eq!(concern_of(&r, NOW), Some(Concern::Failed));
    r.state = WorkspaceState::Running;
    assert_eq!(concern_of(&r, NOW), Some(Concern::Orphan));
}
