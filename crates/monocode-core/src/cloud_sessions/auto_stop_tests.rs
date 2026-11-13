//! The auto-stop policy: the decision engine on a fake clock, and the worker that stops through
//! a fake sandbox backend. No gh, no sbx, no clock is touched.
use super::auto_stop::*;
use super::*;
use crate::codespaces::{GhOutput, GhRunner};

const MIN: u64 = 60;

fn sandbox() -> CloudRef {
    CloudRef::sandbox("monoptah-1")
}
fn session(id: i64) -> SessionFacts {
    SessionFacts {
        id,
        ..Default::default()
    }
}
fn workspace(cloud: CloudRef, sessions: Vec<SessionFacts>) -> WorkspaceFacts {
    WorkspaceFacts {
        cloud,
        state: CloudState::Available,
        sessions,
        app_working: false,
    }
}
fn running() -> Vec<WorkspaceFacts> {
    vec![workspace(sandbox(), vec![session(1)])]
}
fn prefs() -> AutoStopPrefs {
    AutoStopPrefs::default()
}
/// Checks once a minute from `from` to `to` (inclusive, in minutes) and returns every decision.
fn run(
    engine: &mut AutoStop,
    prefs: &AutoStopPrefs,
    from: u64,
    to: u64,
    facts: &[WorkspaceFacts],
) -> Vec<(u64, Decision)> {
    let mut found = Vec::new();
    for minute in from..=to {
        for decision in engine.check(minute * MIN, prefs, facts) {
            found.push((minute, decision));
        }
    }
    found
}

#[test]
fn defaults_stop_a_sandbox_after_ten_minutes_and_leave_codespaces_alone() {
    let prefs = prefs();
    assert_eq!(prefs.sandbox.idle_minutes, Some(10));
    assert!(prefs.sandbox.stop_when_unused);
    assert_eq!(prefs.codespace.idle_minutes, None);
    assert!(!prefs.codespace.stop_when_unused);
    // A saved file from before the setting existed, or an empty object, gives the defaults.
    assert_eq!(serde_json::from_str::<AutoStopPrefs>("{}").unwrap(), prefs);
    let partial: AutoStopPrefs = serde_json::from_str(r#"{"sandbox":{"idleMinutes":30}}"#).unwrap();
    assert_eq!(partial.sandbox.idle_minutes, Some(30));
    assert_eq!(partial.codespace, prefs.codespace);
}

#[test]
fn crossing_the_idle_threshold_gives_exactly_one_stop() {
    let mut engine = AutoStop::new();
    let found = run(&mut engine, &prefs(), 0, 14, &running());
    // The first check starts the clock, so ten idle minutes later it fires, once, even though
    // the cached state keeps saying Available.
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, 10);
    assert_eq!(found[0].1.cloud, sandbox());
    assert_eq!(found[0].1.reason, Reason::Idle { minutes: 10 });
}

#[test]
fn a_busy_session_keeps_the_workspace_and_restarts_the_clock() {
    let mut engine = AutoStop::new();
    let mut busy = running();
    busy[0].sessions[0].busy = true;
    assert!(run(&mut engine, &prefs(), 0, 120, &busy).is_empty());
    // The turn ended at minute 120: ten idle minutes are counted from there.
    let found = run(&mut engine, &prefs(), 121, 134, &running());
    assert_eq!(found.len(), 1);
    assert!(found[0].0 >= 130, "{found:?}");
}

#[test]
fn anything_pending_or_unknown_is_not_idle() {
    for change in [
        |s: &mut SessionFacts| s.pending = true,
        |s: &mut SessionFacts| s.unknown = true,
    ] {
        let mut engine = AutoStop::new();
        let mut facts = running();
        change(&mut facts[0].sessions[0]);
        assert!(run(&mut engine, &prefs(), 0, 240, &facts).is_empty());
    }
    // The app archiving, converting or moving something on the workspace.
    let mut engine = AutoStop::new();
    let mut facts = running();
    facts[0].app_working = true;
    assert!(run(&mut engine, &prefs(), 0, 240, &facts).is_empty());
    // One busy session among several is enough.
    let mut engine = AutoStop::new();
    let mut facts = vec![workspace(sandbox(), vec![session(1), session(2)])];
    facts[0].sessions[1].busy = true;
    assert!(run(&mut engine, &prefs(), 0, 240, &facts).is_empty());
}

#[test]
fn an_unknown_workspace_state_is_never_stopped() {
    for state in [
        CloudState::Unknown,
        CloudState::Starting,
        CloudState::Stopping,
        CloudState::Stopped,
        CloudState::Unavailable,
    ] {
        let mut engine = AutoStop::new();
        let mut facts = running();
        facts[0].state = state;
        assert!(
            run(&mut engine, &prefs(), 0, 240, &facts).is_empty(),
            "{state:?}"
        );
    }
}

#[test]
fn user_input_counts_as_activity() {
    let mut engine = AutoStop::new();
    let mut facts = running();
    // Typing at minute 8 pushes the stop to minute 18.
    facts[0].sessions[0].last_input = Some(8 * MIN);
    let found = run(&mut engine, &prefs(), 0, 22, &facts);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, 18);
    // An input time in the future (clock skew) reads as "now": never idle, never stopped.
    let mut engine = AutoStop::new();
    facts[0].sessions[0].last_input = Some(10_000 * MIN);
    assert!(run(&mut engine, &prefs(), 0, 60, &facts).is_empty());
}

#[test]
fn a_disabled_setting_never_stops() {
    let mut off = prefs();
    off.sandbox.idle_minutes = None;
    off.sandbox.stop_when_unused = false;
    assert!(!off.any_enabled());
    let mut engine = AutoStop::new();
    assert!(run(&mut engine, &off, 0, 600, &running()).is_empty());
    // And with nothing left on the workspace either.
    assert!(run(&mut engine, &off, 601, 1200, &[]).is_empty());
}

#[test]
fn codespaces_are_off_by_default_and_follow_their_own_setting_when_on() {
    let facts = vec![workspace(CloudRef::codespace("octo-abc"), vec![session(1)])];
    let mut engine = AutoStop::new();
    assert!(run(&mut engine, &prefs(), 0, 600, &facts).is_empty());
    let mut on = prefs();
    on.codespace.idle_minutes = Some(15);
    let mut engine = AutoStop::new();
    let found = run(&mut engine, &on, 0, 19, &facts);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, 15);
    assert!(found[0].1.notice().contains("codespace octo-abc"));
}

#[test]
fn archiving_the_last_session_stops_the_workspace_after_the_grace() {
    let mut engine = AutoStop::new();
    assert!(run(&mut engine, &prefs(), 0, 3, &running()).is_empty());
    // The last session is archived at minute 4: the workspace has no session in the window.
    let found = run(&mut engine, &prefs(), 4, 60, &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].1.cloud, sandbox());
    assert_eq!(found[0].1.reason, Reason::Unused);
    assert_eq!(found[0].0, 14);
    // Done with it, nothing more is asked for the same workspace.
    engine.finished(&found[0].1, &StopOutcome::Stopped, found[0].0 * MIN);
    assert!(run(&mut engine, &prefs(), 61, 600, &[]).is_empty());
}

#[test]
fn the_unused_rule_is_off_for_codespaces_and_when_unchecked() {
    let mut engine = AutoStop::new();
    let codespace = vec![workspace(CloudRef::codespace("octo-abc"), vec![session(1)])];
    assert!(run(&mut engine, &prefs(), 0, 3, &codespace).is_empty());
    assert!(run(&mut engine, &prefs(), 4, 600, &[]).is_empty());

    let mut unchecked = prefs();
    unchecked.sandbox.stop_when_unused = false;
    let mut engine = AutoStop::new();
    assert!(run(&mut engine, &unchecked, 0, 3, &running()).is_empty());
    assert!(run(&mut engine, &unchecked, 4, 600, &[]).is_empty());
}

#[test]
fn a_workspace_never_seen_with_a_session_is_not_an_orphan_to_stop() {
    // The overview lists orphans; this policy only knows workspaces that had a session here.
    let mut engine = AutoStop::new();
    assert!(run(&mut engine, &prefs(), 0, 600, &[]).is_empty());
}

#[test]
fn an_already_stopped_workspace_is_not_stopped_again_and_resume_starts_a_new_window() {
    let mut engine = AutoStop::new();
    let found = run(&mut engine, &prefs(), 0, 12, &running());
    assert_eq!(found.len(), 1);
    engine.finished(&found[0].1, &StopOutcome::Stopped, found[0].0 * MIN);
    // The state now reads Stopped for hours: no further call.
    let mut stopped = running();
    stopped[0].state = CloudState::Stopped;
    assert!(run(&mut engine, &prefs(), 21, 300, &stopped).is_empty());
    // The user resumes it with a send. The idle window starts from there.
    engine.resumed(&sandbox(), 301 * MIN);
    let again = run(&mut engine, &prefs(), 301, 315, &running());
    assert_eq!(again.len(), 1);
    assert!(again[0].0 >= 311, "{again:?}");
}

#[test]
fn resuming_before_the_state_was_seen_stopped_does_not_disable_the_policy() {
    let mut engine = AutoStop::new();
    let found = run(&mut engine, &prefs(), 0, 10, &running());
    assert_eq!(found.len(), 1);
    engine.finished(&found[0].1, &StopOutcome::Stopped, 10 * MIN);
    // Stopped and resumed inside one check interval: the state never read Stopped.
    engine.resumed(&sandbox(), 10 * MIN + 20);
    let again = run(&mut engine, &prefs(), 11, 25, &running());
    assert_eq!(again.len(), 1, "{again:?}");
}

#[test]
fn a_stop_the_state_never_confirmed_is_forgotten_after_a_while() {
    let mut engine = AutoStop::new();
    // Asked for at minute 10; the cache keeps saying Available (say the call was lost).
    let found = run(&mut engine, &prefs(), 0, 40, &running());
    assert!(found.len() >= 2, "{found:?}");
    assert!(found[1].0 - found[0].0 >= 5, "{found:?}");
}

#[test]
fn a_refused_or_failed_stop_waits_a_whole_window_before_trying_again() {
    for outcome in [StopOutcome::InUse, StopOutcome::Failed("boom".into())] {
        let mut engine = AutoStop::new();
        let first = run(&mut engine, &prefs(), 0, 10, &running());
        assert_eq!(first.len(), 1);
        engine.finished(&first[0].1, &outcome, 10 * MIN);
        assert!(run(&mut engine, &prefs(), 11, 19, &running()).is_empty());
        assert_eq!(run(&mut engine, &prefs(), 20, 21, &running()).len(), 1);
    }
}

#[test]
fn the_notices_name_the_workspace_and_say_how_to_resume() {
    let idle = Decision {
        cloud: sandbox(),
        reason: Reason::Idle { minutes: 10 },
    };
    assert_eq!(
        idle.notice(),
        "Stopped Docker sandbox monoptah-1 after 10 min idle; send a message to resume."
    );
    let unused = Decision {
        cloud: sandbox(),
        reason: Reason::Unused,
    };
    assert!(unused
        .notice()
        .contains("last session was archived or deleted"));
}

#[test]
fn an_absurd_idle_time_is_brought_into_range() {
    let mut odd = prefs();
    odd.sandbox.idle_minutes = Some(0);
    assert_eq!(
        odd.policy(CloudBackendKind::DockerSandbox).idle_minutes,
        Some(1)
    );
    odd.sandbox.idle_minutes = Some(u32::MAX);
    assert_eq!(
        odd.policy(CloudBackendKind::DockerSandbox).idle_minutes,
        Some(24 * 60)
    );
}

// --- the worker -----------------------------------------------------------------------

struct NoGh;
impl GhRunner for NoGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        panic!("gh must not be called: {args:?}");
    }
}
struct NoHost;
impl Host for NoHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(vec![])
    }
    fn connect(
        &self,
        _: &str,
        _: &str,
        _: &AtomicBool,
        _: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        Err("unused".into())
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        Err("a background stop must never wake anything".into())
    }
    fn request(&self, _: &str, _: &str, _: Value) -> Result<Value, String> {
        Err("unused".into())
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A sandbox backend that remembers its state and every call.
struct Scripted {
    state: Mutex<String>,
    calls: Mutex<Vec<String>>,
}
impl Scripted {
    fn new(state: &str) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state.into()),
            calls: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn stops(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.starts_with("stop"))
            .count()
    }
}
impl CloudBackend for Scripted {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::DockerSandbox
    }
    fn display_state(&self, raw: &str) -> CloudState {
        match raw {
            "Available" => CloudState::Available,
            "Stopped" => CloudState::Stopped,
            _ => CloudState::Unknown,
        }
    }
    fn is_stopped(&self, raw: &str) -> bool {
        raw == "Stopped"
    }
    fn is_deleted_state(&self, raw: &str) -> bool {
        raw == "Removed"
    }
    fn is_missing_error(&self, error: &str) -> bool {
        error.contains("no such sandbox")
    }
    fn state(&self, name: &str) -> Result<String, String> {
        self.calls.lock().unwrap().push(format!("state {name}"));
        Ok(self.state.lock().unwrap().clone())
    }
    fn exists(&self, _: &str) -> Result<bool, String> {
        Ok(true)
    }
    fn stop(&self, name: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("stop {name}"));
        *self.state.lock().unwrap() = "Stopped".into();
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        panic!("auto-stop must never delete {name}");
    }
    fn rebuild(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn web_url(&self, _: &str) -> Option<Result<String, String>> {
        None
    }
    fn run_script(&self, _: &str, _: &str, _: bool) -> Result<String, String> {
        Ok(String::new())
    }
    fn copy_from(&self, _: &str, _: &str, _: &std::path::Path) -> Result<(), String> {
        Ok(())
    }
    fn prepare_evidence(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn world(state: &str, with_session: bool) -> (CloudSessions, Arc<Scripted>) {
    let store = Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let backend = Scripted::new(state);
    let svc =
        CloudSessions::new(Arc::new(NoGh), Arc::new(NoHost), store).with_backend(backend.clone());
    if with_session {
        let project = crate::store::pending_cloud_project(
            "/Users/me/widgets",
            &sandbox(),
            "/workspaces/widgets",
        );
        svc.store()
            .unwrap()
            .create_pending_cloud_session("claude", project)
            .unwrap();
    }
    (svc, backend)
}
fn idle() -> Decision {
    Decision {
        cloud: sandbox(),
        reason: Reason::Idle { minutes: 10 },
    }
}
fn unused() -> Decision {
    Decision {
        cloud: sandbox(),
        reason: Reason::Unused,
    }
}

#[test]
fn the_worker_stops_a_running_idle_workspace_with_one_call_and_never_deletes_or_wakes() {
    let (svc, backend) = world("Available", true);
    assert_eq!(stop_if_still_idle(&svc, &idle()), StopOutcome::Stopped);
    assert_eq!(backend.stops(), 1);
    // The next decision for the same workspace finds it stopped: no second stop call.
    assert_eq!(stop_if_still_idle(&svc, &idle()), StopOutcome::NotRunning);
    assert_eq!(backend.stops(), 1);
    assert_eq!(svc.cloud_state_of(&sandbox()), CloudState::Stopped);
}

#[test]
fn the_worker_leaves_a_stopped_or_unreadable_workspace_alone() {
    for state in ["Stopped", "Broken"] {
        let (svc, backend) = world(state, true);
        assert_eq!(stop_if_still_idle(&svc, &idle()), StopOutcome::NotRunning);
        assert_eq!(backend.stops(), 0, "{state}");
    }
}

#[test]
fn the_worker_refuses_an_unused_stop_while_a_session_still_uses_the_workspace() {
    // The window forgot it (a closed project), but the store still has a session on it.
    let (svc, backend) = world("Available", true);
    assert_eq!(stop_if_still_idle(&svc, &unused()), StopOutcome::InUse);
    assert_eq!(backend.stops(), 0);
    // With none left, it stops.
    let (svc, backend) = world("Available", false);
    assert_eq!(stop_if_still_idle(&svc, &unused()), StopOutcome::Stopped);
    assert_eq!(backend.stops(), 1);
}
