//! The shared-instance guard and the missing-instance rule, on a fake backend: no gh, no sbx.
use super::instance_action::*;
use super::*;
use crate::codespaces::{GhOutput, GhRunner};
use std::sync::atomic::AtomicBool;

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
        Err("unused".into())
    }
    fn request(&self, _: &str, _: &str, _: Value) -> Result<Value, String> {
        Err("unused".into())
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Remembers every call; `stop` and `delete` answer what the test scripted.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    stop_error: Mutex<Option<String>>,
    delete_error: Mutex<Option<String>>,
}
impl Fake {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}
impl CloudBackend for Fake {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::DockerSandbox
    }
    fn display_state(&self, _: &str) -> CloudState {
        CloudState::Available
    }
    fn is_stopped(&self, _: &str) -> bool {
        false
    }
    fn is_deleted_state(&self, _: &str) -> bool {
        false
    }
    fn is_missing_error(&self, error: &str) -> bool {
        error.contains("no such sandbox")
    }
    fn state(&self, _: &str) -> Result<String, String> {
        Ok("Available".into())
    }
    fn exists(&self, _: &str) -> Result<bool, String> {
        Ok(true)
    }
    fn stop(&self, name: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("stop {name}"));
        self.stop_error.lock().unwrap().clone().map_or(Ok(()), Err)
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("delete {name}"));
        self.delete_error
            .lock()
            .unwrap()
            .clone()
            .map_or(Ok(()), Err)
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

fn cloud() -> CloudRef {
    CloudRef::sandbox("monoptah-1")
}

fn world() -> (CloudSessions, Arc<Fake>) {
    let store = Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let fake = Arc::new(Fake::default());
    let svc =
        CloudSessions::new(Arc::new(NoGh), Arc::new(NoHost), store).with_backend(fake.clone());
    (svc, fake)
}

/// A session of the project on the instance; `busy` marks a turn running in it.
fn add_session(svc: &CloudSessions, busy: bool) -> i64 {
    let project =
        crate::store::pending_cloud_project("/Users/me/widgets", &cloud(), "/workspaces/widgets");
    let store = svc.store().unwrap();
    let summary = store
        .create_pending_cloud_session("claude", project)
        .unwrap();
    if busy {
        let mut remote = store.remote_session(summary.id).unwrap().unwrap();
        remote.running = true;
        store.save_remote_session(&remote).unwrap();
    }
    summary.id
}

fn live(id: i64, title: &str, busy: bool) -> InstanceSession {
    InstanceSession {
        id,
        title: title.into(),
        local_project: None,
        running: busy,
        needs_input: false,
        pending: false,
        starting: false,
        updated_at: 0,
    }
}

#[test]
fn delete_is_refused_while_any_other_live_session_uses_the_instance() {
    let idle = [live(2, "Other", false)];
    assert_eq!(
        plan_instance_action(InstanceAction::Delete, &idle),
        InstancePlan::Refuse {
            sharing: vec!["Other".into()]
        }
    );
    assert_eq!(
        plan_instance_action(InstanceAction::Delete, &[]),
        InstancePlan::Proceed
    );
}

#[test]
fn stop_holds_back_only_for_a_busy_session() {
    let idle = [live(2, "Idle", false)];
    assert_eq!(
        plan_instance_action(InstanceAction::Stop, &idle),
        InstancePlan::Proceed
    );
    let mixed = [live(2, "Idle", false), live(3, "Working", true)];
    assert_eq!(
        plan_instance_action(InstanceAction::Stop, &mixed),
        InstancePlan::LeaveRunning {
            busy: vec!["Working".into()]
        }
    );
    let mut asking = live(4, "", false);
    asking.needs_input = true;
    assert_eq!(
        plan_instance_action(InstanceAction::Stop, &[asking]),
        InstancePlan::LeaveRunning {
            busy: vec!["Untitled".into()]
        }
    );
}

#[test]
fn a_shared_instance_is_never_deleted_and_the_backend_is_not_even_asked() {
    let (svc, fake) = world();
    let mine = add_session(&svc, false);
    let other = add_session(&svc, false);
    let error = svc
        .apply_instance_action(&cloud(), InstanceAction::Delete, &[mine])
        .unwrap_err();
    assert!(error.contains("not deleted"), "{error}");
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    // Once the other session is part of the same action, the instance can go.
    assert!(matches!(
        svc.apply_instance_action(&cloud(), InstanceAction::Delete, &[mine, other])
            .unwrap(),
        InstanceOutcome::Deleted(_)
    ));
    assert_eq!(fake.calls(), vec!["delete monoptah-1"]);
}

#[test]
fn an_archived_other_session_does_not_share_the_instance() {
    let (svc, fake) = world();
    let mine = add_session(&svc, false);
    let other = add_session(&svc, false);
    svc.store().unwrap().set_archived(other, true).unwrap();
    assert!(matches!(
        svc.apply_instance_action(&cloud(), InstanceAction::Delete, &[mine])
            .unwrap(),
        InstanceOutcome::Deleted(_)
    ));
    assert_eq!(fake.calls(), vec!["delete monoptah-1"]);
}

#[test]
fn stop_leaves_the_instance_running_under_a_busy_session_and_says_so() {
    let (svc, fake) = world();
    let mine = add_session(&svc, false);
    add_session(&svc, true);
    match svc
        .apply_instance_action(&cloud(), InstanceAction::Stop, &[mine])
        .unwrap()
    {
        InstanceOutcome::LeftRunning(note) => assert!(note.contains("left running"), "{note}"),
        other => panic!("{other:?}"),
    }
    assert!(fake.calls().is_empty());
}

#[test]
fn stop_goes_ahead_when_the_others_are_idle() {
    let (svc, fake) = world();
    let mine = add_session(&svc, false);
    add_session(&svc, false);
    assert_eq!(
        svc.apply_instance_action(&cloud(), InstanceAction::Stop, &[mine])
            .unwrap(),
        InstanceOutcome::Stopped
    );
    assert_eq!(fake.calls(), vec!["stop monoptah-1"]);
}

#[test]
fn stopping_or_deleting_an_instance_that_no_longer_exists_is_a_success() {
    let (svc, fake) = world();
    let mine = add_session(&svc, false);
    *fake.stop_error.lock().unwrap() = Some("error: no such sandbox: monoptah-1".into());
    *fake.delete_error.lock().unwrap() = Some("error: no such sandbox: monoptah-1".into());
    assert_eq!(
        svc.apply_instance_action(&cloud(), InstanceAction::Stop, &[mine])
            .unwrap(),
        InstanceOutcome::Stopped
    );
    assert!(svc.cloud_gone(&cloud()));
    // Known to be gone now: the delete calls nothing and is a success too.
    assert_eq!(
        svc.apply_instance_action(&cloud(), InstanceAction::Delete, &[mine])
            .unwrap(),
        InstanceOutcome::AlreadyGone
    );
    // A delete that finds it missing on its own is a success as well.
    let (svc, fake) = world();
    let mine = add_session(&svc, false);
    *fake.delete_error.lock().unwrap() = Some("error: no such sandbox: monoptah-1".into());
    assert!(matches!(
        svc.apply_instance_action(&cloud(), InstanceAction::Delete, &[mine])
            .unwrap(),
        InstanceOutcome::Deleted(_)
    ));
}

#[test]
fn other_stop_and_delete_errors_are_still_errors() {
    let (svc, fake) = world();
    let mine = add_session(&svc, false);
    *fake.stop_error.lock().unwrap() = Some("sbx: connection refused".into());
    *fake.delete_error.lock().unwrap() = Some("sbx: permission denied".into());
    assert_eq!(
        svc.stop_cloud(&cloud()).unwrap_err(),
        "sbx: connection refused"
    );
    assert_eq!(
        svc.apply_instance_action(&cloud(), InstanceAction::Delete, &[mine])
            .unwrap_err(),
        "sbx: permission denied"
    );
    assert!(!svc.cloud_gone(&cloud()));
}

#[test]
fn only_a_proven_absence_counts_as_missing() {
    use backend::codespaces::CodespacesBackend;
    use backend::docker::DockerBackend;
    let gh = Arc::new(NoGh);
    let codespaces = CodespacesBackend::new(gh);
    assert!(codespaces.is_missing_error("error stopping codespace: HTTP 404: Not Found"));
    assert!(codespaces.is_missing_error("Codespace not found"));
    assert!(!codespaces.is_missing_error("gh: command not found"));
    assert!(!codespaces.is_missing_error("HTTP 500: Server Error"));
    // The same words inside a longer message prove nothing.
    assert!(codespaces.is_missing_error("error: Codespace not found"));
    assert!(codespaces.is_missing_error("no such codespace: octo-abc"));
    assert!(!codespaces.is_missing_error(
        "gh: could not resolve host; the codespace not found in cache, try again"
    ));
    assert!(!codespaces.is_missing_error(
        "authentication failed: token cannot see this codespace, no such codespace for your account scope"
    ));
    assert!(!codespaces.is_missing_error("dial tcp: i/o timeout (codespace not found?)"));
    let docker = DockerBackend::new(Arc::new(crate::sandboxes::SystemSbx));
    assert!(docker.is_missing_error("error: sandbox 'sbx_1' not found"));
    assert!(docker.is_missing_error("error: no such sandbox: sbx_1"));
    assert!(docker.is_missing_error("Sandbox monoptah-1 was not found"));
    assert!(!docker.is_missing_error("error: template 'x' not found"));
    assert!(!docker.is_missing_error("sbx: command not found"));
}
