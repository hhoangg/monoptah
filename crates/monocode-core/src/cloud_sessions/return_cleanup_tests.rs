//! Deleting the instance after "Continue locally": arming, the guards and the retry rules.
//! Everything is fake: a spy backend records the delete (nothing real is created or removed)
//! and the Host answers from a list the test sets.
use super::continue_tests::{encoded, moved, summary_provider};
use super::convert_tests::{world, world_with, SyncHost, World, SESSION};
use super::move_session::{MoveRequest, MoveTarget, StartedBy};
use super::return_cleanup::{ReturnSource, ALREADY_DELETING};
use super::startup::save_start_record;
use super::*;
use crate::remote::{Machine, SandboxTarget};
use crate::store::SessionLocation;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Weak;

type Listing = Result<Vec<(String, String)>, String>;

/// The Host of the workspace: answers the session listing from `list`, counts wakes, and
/// forwards everything else to the fixture's Host.
struct TestHost {
    inner: Arc<SyncHost>,
    extra: Vec<Machine>,
    list: Mutex<Listing>,
    ensures: AtomicUsize,
    disconnected: Mutex<Vec<String>>,
}
impl Host for TestHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        let mut all = self.inner.machines()?;
        all.extend(self.extra.clone());
        Ok(all)
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
    fn ensure(
        &self,
        id: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        self.ensures.fetch_add(1, Ordering::SeqCst);
        self.inner.ensure(id, cancel, progress)
    }
    fn request(&self, id: &str, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "projects.list" => self
                .list
                .lock()
                .unwrap()
                .clone()
                .map(|_| json!([{"id": "p"}])),
            "sessions.list" => self.list.lock().unwrap().clone().map(|sessions| {
                Value::Array(
                    sessions
                        .iter()
                        .map(|(id, title)| json!({"id": id, "title": title}))
                        .collect(),
                )
            }),
            _ => self.inner.request(id, method, params),
        }
    }
    fn disconnect(&self, id: &str) -> Result<(), String> {
        self.disconnected.lock().unwrap().push(id.into());
        Ok(())
    }
}

type Hook = Arc<dyn Fn() + Send + Sync>;

/// A backend that records `delete` instead of running it and forwards the rest to the real
/// backend of the fixture (codespaces) or answers trivially (sandbox).
struct Spy {
    kind: CloudBackendKind,
    inner: Option<Arc<dyn CloudBackend>>,
    calls: AtomicUsize,
    deleted: Mutex<Vec<String>>,
    fail_next: Mutex<Option<String>>,
    hook: Mutex<Option<Hook>>,
    /// Runs inside `exists`, which the cleanup calls after the Host listing.
    exists_hook: Mutex<Option<Hook>>,
    exists: AtomicBool,
    /// `exists` fails with this (offline).
    exists_error: Mutex<Option<String>>,
}
impl Spy {
    fn new(kind: CloudBackendKind, inner: Option<Arc<dyn CloudBackend>>) -> Arc<Self> {
        Arc::new(Self {
            kind,
            inner,
            calls: AtomicUsize::new(0),
            deleted: Mutex::new(vec![]),
            fail_next: Mutex::new(None),
            hook: Mutex::new(None),
            exists_hook: Mutex::new(None),
            exists: AtomicBool::new(true),
            exists_error: Mutex::new(None),
        })
    }
    fn deleted(&self) -> Vec<String> {
        self.deleted.lock().unwrap().clone()
    }
    fn set_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Arc::new(hook));
    }
}
impl CloudBackend for Spy {
    fn kind(&self) -> CloudBackendKind {
        self.kind
    }
    fn display_state(&self, raw: &str) -> CloudState {
        self.inner
            .as_ref()
            .map_or(CloudState::Unknown, |b| b.display_state(raw))
    }
    fn is_stopped(&self, raw: &str) -> bool {
        self.inner.as_ref().is_some_and(|b| b.is_stopped(raw))
    }
    fn is_deleted_state(&self, raw: &str) -> bool {
        self.inner.as_ref().is_some_and(|b| b.is_deleted_state(raw))
    }
    fn is_missing_error(&self, error: &str) -> bool {
        error.contains("not found")
    }
    fn state(&self, _: &str) -> Result<String, String> {
        Ok("Available".into())
    }
    fn exists(&self, _: &str) -> Result<bool, String> {
        let hook = self.exists_hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook();
        }
        if let Some(error) = self.exists_error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(self.exists.load(Ordering::SeqCst))
    }
    fn stop(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let hook = self.hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook();
        }
        if let Some(error) = self.fail_next.lock().unwrap().take() {
            return Err(error);
        }
        self.deleted.lock().unwrap().push(name.into());
        Ok(())
    }
    fn rebuild(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn web_url(&self, _: &str) -> Option<Result<String, String>> {
        None
    }
    fn run_script(&self, name: &str, script: &str, long: bool) -> Result<String, String> {
        match &self.inner {
            Some(inner) => inner.run_script(name, script, long),
            None => Ok(String::new()),
        }
    }
    fn copy_from(&self, name: &str, remote: &str, local: &std::path::Path) -> Result<(), String> {
        match &self.inner {
            Some(inner) => inner.copy_from(name, remote, local),
            None => Ok(()),
        }
    }
    fn take_copy_notes(&self) -> Vec<String> {
        self.inner
            .as_ref()
            .map(|b| b.take_copy_notes())
            .unwrap_or_default()
    }
    fn prepare_evidence(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

struct Rig {
    w: World,
    svc: Arc<CloudSessions>,
    host: Arc<TestHost>,
    spy: Arc<Spy>,
    sandbox_spy: Arc<Spy>,
    /// Milliseconds the service's clock reads.
    now: Arc<AtomicU64>,
}

fn sandbox_machine() -> Machine {
    Machine {
        id: "m-sbx".into(),
        name: "sbx".into(),
        endpoint: "sbx://sbx-1".into(),
        environment_id: "env-sbx".into(),
        ssh: None,
        codespace: None,
        sandbox: Some(SandboxTarget {
            id: "sbx_1".into(),
            name: "sbx-1".into(),
            workspace: "/work".into(),
            relay_port: 1,
            config: None,
            mode: Default::default(),
            docker_account: None,
        }),
    }
}

fn rig_with(w: World, list: Listing) -> Rig {
    let host = Arc::new(TestHost {
        inner: w.host.clone(),
        extra: vec![sandbox_machine()],
        list: Mutex::new(list),
        ensures: AtomicUsize::new(0),
        disconnected: Mutex::new(vec![]),
    });
    let spy = Spy::new(
        CloudBackendKind::Codespaces,
        Some(w.svc.backend(CloudBackendKind::Codespaces).unwrap()),
    );
    let sandbox_spy = Spy::new(CloudBackendKind::DockerSandbox, None);
    let now = Arc::new(AtomicU64::new(1_000_000));
    let clock = now.clone();
    let svc = Arc::new(
        CloudSessions::new(w.gh.clone(), host.clone(), w.svc.store.clone())
            .with_timing(Duration::ZERO, |_| {}, move || clock.load(Ordering::SeqCst))
            .with_claude_dir(w.claude.clone())
            .with_workspace_root(&w.root)
            .with_backend(spy.clone())
            .with_backend(sandbox_spy.clone()),
    );
    Rig {
        w,
        svc,
        host,
        spy,
        sandbox_spy,
        now,
    }
}

/// The Host lists only the session that comes back (still there, saved as a copy).
fn own_only() -> Listing {
    Ok(vec![("host-1".into(), "Add made.txt".into())])
}

/// A workspace without ignored files, so nothing is left behind by the return.
fn clean_world() -> World {
    world_with(true, false, false)
}

fn rig() -> Rig {
    rig_with(clean_world(), own_only())
}

impl Rig {
    fn cont(&self, id: i64) -> Result<Converted, ConvertError> {
        self.svc.continue_locally(id, &mut |_| {})
    }
    fn record(&self, local: i64) -> Option<ReturnCleanup> {
        self.svc.return_cleanup_for_session(local)
    }
    fn weak(&self) -> Weak<CloudSessions> {
        Arc::downgrade(&self.svc)
    }
}

fn cs() -> CloudRef {
    CloudRef::codespace("cs-1")
}

fn bound_to(r: &Rig, cloud: &CloudRef) -> bool {
    let store = r.svc.store().unwrap();
    store
        .remote_projects()
        .unwrap()
        .values()
        .any(|p| p.cloud().as_ref() == Some(cloud))
}

#[test]
fn a_moved_return_arms_a_clear_record_and_the_delete_runs_after_everything_is_in_place() {
    let r = rig();
    let local = moved(&r.w);
    let done = r.cont(local).unwrap();
    assert!(
        done.warnings.iter().all(|w| !w.contains("kept")),
        "{:?}",
        done.warnings
    );
    let record = r.record(local).expect("armed");
    assert_eq!(record.host_check, HostCheck::Clear);
    assert_eq!(record.cloud, cs());
    assert_eq!(record.environment_id, "env-1");
    assert_eq!(record.host_session_id.as_deref(), Some("host-1"));
    assert!(record.auto_run());
    assert_eq!(r.svc.return_cleanups().len(), 1);
    assert!(r.spy.deleted().is_empty(), "arming deletes nothing");
    assert!(bound_to(&r, &cs()));

    let weak = r.weak();
    let project = r.w.project.clone();
    let jsonl = encoded(&r.w.claude, &r.w.project).join(format!("{SESSION}.jsonl"));
    r.spy.set_hook(move || {
        let svc = weak.upgrade().unwrap();
        // Order: the return is complete before the instance goes.
        assert_eq!(
            svc.store().unwrap().cloud_location(local).unwrap(),
            SessionLocation::Local
        );
        assert!(std::fs::read_to_string(&jsonl).unwrap().contains("\"u2\""));
        assert_eq!(
            std::fs::read_to_string(project.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        let record = svc.return_cleanup_for_session(local).unwrap();
        assert!(
            record.attempt_started_at.is_some(),
            "recorded before the call"
        );
    });
    let wakes = r.host.ensures.load(Ordering::SeqCst);
    let outcome = r.svc.run_return_cleanup(&cs().key()).unwrap();
    assert_eq!(outcome, CleanupOutcome::Deleted { warning: None });
    assert_eq!(r.spy.deleted(), vec!["cs-1".to_string()]);
    assert!(r.record(local).is_none(), "the record is gone");
    assert!(!bound_to(&r, &cs()), "the bindings are forgotten");
    assert!(r.svc.sessions_on(&cs()).is_empty());
    assert_eq!(
        r.host.ensures.load(Ordering::SeqCst),
        wakes,
        "the delete wakes nothing"
    );
    assert_eq!(
        r.svc.run_return_cleanup(&cs().key()).unwrap(),
        CleanupOutcome::Nothing
    );
}

#[test]
fn a_docker_sandbox_is_cleaned_up_the_same_way() {
    let r = rig();
    let sbx = CloudRef::sandbox("sbx-1");
    let local = {
        let store = r.svc.store().unwrap();
        store
            .remember_cloud_ref_project(
                "env-sbx",
                &json!({"id": "p", "cwd": "/work"}),
                r.w.project.to_str().unwrap(),
                &sbx,
            )
            .unwrap();
        store
            .create("claude", r.w.project.to_str().unwrap())
            .unwrap()
            .id
    };
    let mut converted = hand_made(&r, local);
    r.svc.arm_return_cleanup(
        ReturnSource {
            cloud: sbx.clone(),
            environment_id: "env-sbx".into(),
            host_session_id: Some("host-1".into()),
        },
        &mut converted,
    );
    assert!(converted.warnings.is_empty(), "{:?}", converted.warnings);
    let record = r.record(local).expect("armed for the sandbox");
    assert_eq!(record.key(), "sbx:sbx-1");
    assert_eq!(record.host_check, HostCheck::Clear);
    assert!(bound_to(&r, &sbx));
    let weak = r.weak();
    r.sandbox_spy.set_hook(move || {
        let svc = weak.upgrade().unwrap();
        assert_eq!(
            svc.store().unwrap().cloud_location(local).unwrap(),
            SessionLocation::Local
        );
        assert!(svc
            .return_cleanup_for_session(local)
            .unwrap()
            .attempt_started_at
            .is_some());
    });
    assert_eq!(
        r.svc.run_return_cleanup("sbx:sbx-1").unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );
    assert_eq!(r.sandbox_spy.deleted(), vec!["sbx-1".to_string()]);
    assert!(
        r.spy.deleted().is_empty(),
        "the codespace backend is not touched"
    );
    assert_eq!(
        *r.host.disconnected.lock().unwrap(),
        vec!["m-sbx".to_string()]
    );
    assert!(r.record(local).is_none());
    assert!(!bound_to(&r, &sbx));
}

/// A finished return as `convert` reports it, for a session `local` that exists in the store.
fn hand_made(r: &Rig, local: i64) -> Converted {
    let summary = r
        .svc
        .store()
        .unwrap()
        .sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id == local)
        .unwrap();
    Converted {
        cloud_id: 0,
        local_id: local,
        title: "Add made.txt".into(),
        summary,
        codespace: "sbx-1".into(),
        rows: 0,
        resume: Resume::Missing("test".into()),
        evidence: vec![],
        code_dir: None,
        ignored: vec![],
        ignored_truncated: false,
        evidence_partial: false,
        warnings: vec![],
        transcript_lost: false,
        history_saved: true,
    }
}

#[test]
fn a_cloud_born_return_arms_with_the_new_local_session() {
    let r = rig();
    let done = r.cont(r.w.cloud_id).unwrap();
    assert_ne!(done.local_id, r.w.cloud_id);
    let record = r.record(done.local_id).expect("armed for the new session");
    assert_eq!(record.local_id, done.local_id);
    assert_eq!(record.host_check, HostCheck::Clear);
    assert_eq!(record.host_session_id.as_deref(), Some("host-1"));
    assert_eq!(
        r.svc.run_return_cleanup(&record.key()).unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );
    assert_eq!(r.spy.deleted(), vec!["cs-1".to_string()]);
    assert!(r.record(done.local_id).is_none());
}

#[test]
fn a_failed_return_or_one_that_lost_the_workspace_arms_nothing() {
    // A conversion that fails.
    let r = rig();
    let local = moved(&r.w);
    *r.w.gh.fail_snapshot.lock().unwrap() = Some("ssh dropped".into());
    assert!(r.cont(local).is_err());
    assert!(r.svc.return_cleanups().is_empty());
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);

    // A turn running in the cloud.
    {
        let store = r.svc.store().unwrap();
        let (_, mut record) = store.linked_remote_session(local).unwrap().unwrap();
        record.running = true;
        record.flags_at = crate::time::now_ms();
        store.save_remote_session(&record).unwrap();
    }
    let error = r.cont(local).unwrap_err().to_string();
    assert_eq!(error, crate::store::HOST_TURN_RUNNING);
    assert!(r.svc.return_cleanups().is_empty());

    // The workspace is gone: the cloud code was not brought back, the instance stays.
    let lost = rig();
    let local = moved(&lost.w);
    lost.svc
        .store()
        .unwrap()
        .forget_cloud_session(lost.w.cloud_id)
        .unwrap();
    let done = lost.cont(local).unwrap();
    assert!(done.warnings.iter().any(|w| w.contains("gone")));
    assert!(lost.svc.return_cleanups().is_empty());
    assert_eq!(lost.spy.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_delete_error_keeps_the_record_and_a_retry_deletes_exactly_once() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    *r.spy.fail_next.lock().unwrap() = Some("gh is offline".into());
    // A second run while the first is inside the delete is refused.
    let nested: Arc<Mutex<Option<Result<CleanupOutcome, String>>>> = Arc::new(Mutex::new(None));
    let (weak, slot) = (r.weak(), nested.clone());
    r.spy.set_hook(move || {
        let svc = weak.upgrade().unwrap();
        *slot.lock().unwrap() = Some(svc.run_return_cleanup("cs-1"));
    });
    assert_eq!(
        r.svc.run_return_cleanup("cs-1"),
        Err("gh is offline".to_string())
    );
    assert_eq!(
        nested.lock().unwrap().take(),
        Some(Err(ALREADY_DELETING.to_string()))
    );
    let record = r.record(local).expect("kept for a retry");
    assert_eq!(record.last_error.as_deref(), Some("gh is offline"));
    assert!(record.attempt_started_at.is_none());
    assert!(record.needs_retry() && !record.auto_run());
    assert!(r.spy.deleted().is_empty());
    assert!(bound_to(&r, &cs()), "nothing was forgotten");

    // The problem is gone: the retry deletes.
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );
    assert_eq!(r.spy.deleted(), vec!["cs-1".to_string()], "exactly once");
    assert!(r.record(local).is_none());
    assert!(!bound_to(&r, &cs()));
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Nothing
    );
    assert_eq!(r.spy.deleted().len(), 1);
}

#[test]
fn an_interrupted_delete_becomes_a_retry_that_does_not_run_by_itself() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let mut record = r.record(local).unwrap();
    // The app closed while the delete was running.
    record.attempt_started_at = Some(5);
    r.svc
        .store()
        .unwrap()
        .save_setting(
            &format!("cloud-return-cleanup:{}", record.key()),
            &serde_json::to_string(&record).unwrap(),
        )
        .unwrap();
    assert!(!r.record(local).unwrap().auto_run());
    r.svc.mark_interrupted_return_cleanups();
    let after = r.record(local).unwrap();
    assert!(after.attempt_started_at.is_none());
    assert!(
        after
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("app closed") && e.contains("Retry")),
        "{:?}",
        after.last_error
    );
    assert!(
        after.needs_retry() && !after.auto_run(),
        "not run by itself"
    );
    assert!(r.spy.deleted().is_empty());
    // A record that was never tried is left alone.
    let fresh = rig();
    let local = moved(&fresh.w);
    fresh.cont(local).unwrap();
    fresh.svc.mark_interrupted_return_cleanups();
    assert!(fresh.record(local).unwrap().auto_run());
}

#[test]
fn another_session_on_the_instance_keeps_it_and_is_named() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    // Another conversation of the same instance appeared meanwhile.
    let sibling = {
        let store = r.svc.store().unwrap();
        let project = store
            .remember_cloud_project(
                "env-1",
                &json!({"id": "p", "cwd": r.w.ws.to_str().unwrap()}),
                r.w.project.to_str().unwrap(),
                "cs-1",
            )
            .unwrap();
        store
            .register_remote_summary(
                &project,
                &json!({"projectId":"p","revision":1,"status":"idle","updatedAt":5,
                "session":{"id":"host-9","harness":"claude","title":"Sibling work",
                "model":"claude:sonnet","runtimeMode":"auto","blocks":[]}}),
            )
            .unwrap()
            .id
    };
    let CleanupOutcome::Kept { reason } = r.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("Sibling work"), "{reason}");
    assert!(reason.contains("was kept because"), "{reason}");
    assert!(reason.contains("archive dialog"), "{reason}");
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0, "never deleted");
    assert!(r.record(local).is_none(), "no retry loop");
    assert!(r
        .svc
        .store()
        .unwrap()
        .remote_session(sibling)
        .unwrap()
        .is_some());
}

#[test]
fn a_session_located_in_the_instance_keeps_it() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    // Another local session lives in the same instance.
    let other = {
        let store = r.svc.store().unwrap();
        let other = store
            .create("claude", r.w.project.to_str().unwrap())
            .unwrap()
            .id;
        store
            .set_cloud_location(
                other,
                &SessionLocation::Cloud(crate::store::CloudLocation {
                    cloud: cs(),
                    environment_id: "env-1".into(),
                    project_id: "p".into(),
                    host_session_id: "host-7".into(),
                    remote_cwd: "/w".into(),
                    local_cwd: r.w.project.to_str().unwrap().into(),
                    moved_at_row: 0,
                    pushed_branch: None,
                }),
            )
            .unwrap();
        other
    };
    let CleanupOutcome::Kept { reason } = r.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("running in it"), "{reason}");
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
    assert!(r
        .svc
        .store()
        .unwrap()
        .cloud_location(other)
        .unwrap()
        .is_cloud());
}

#[test]
fn another_live_host_session_keeps_it_unless_it_was_saved_as_a_copy() {
    let both = vec![
        ("host-1".to_string(), "Add made.txt".to_string()),
        ("host-2".to_string(), "Other work".to_string()),
    ];
    let r = rig_with(clean_world(), Ok(both.clone()));
    let local = moved(&r.w);
    r.cont(local).unwrap();
    assert_eq!(
        r.record(local).unwrap().host_check,
        HostCheck::Others(vec!["Other work".into()])
    );
    let CleanupOutcome::Kept { reason } = r.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("Other work"), "{reason}");
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
    assert!(r.record(local).is_none());

    // The same listing, but the other session was already saved as a local copy.
    let r = rig_with(clean_world(), Ok(both));
    r.svc
        .store()
        .unwrap()
        .set_cloud_converted("env-1", "host-2", true)
        .unwrap();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    assert_eq!(r.record(local).unwrap().host_check, HostCheck::Clear);
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );
}

#[test]
fn an_unknown_host_state_fails_closed_and_can_be_retried() {
    let r = rig_with(clean_world(), Err("tunnel down".into()));
    let local = moved(&r.w);
    r.cont(local).unwrap();
    assert!(
        matches!(r.record(local).unwrap().host_check, HostCheck::Unknown(ref e) if e.contains("tunnel down"))
    );
    let error = r.svc.run_return_cleanup("cs-1").unwrap_err();
    assert!(
        error.contains("could not check whether other sessions use"),
        "{error}"
    );
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0, "no delete");
    let record = r.record(local).expect("kept for a retry");
    assert!(record.needs_retry() && !record.auto_run());
    // The Host answers now: the retry checks live and deletes.
    *r.host.list.lock().unwrap() = own_only();
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );
    assert_eq!(r.spy.deleted(), vec!["cs-1".to_string()]);
}

#[test]
fn a_clear_record_deletes_a_stopped_instance_without_waking_it() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    assert_eq!(r.record(local).unwrap().host_check, HostCheck::Clear);
    // The instance went to sleep: the Host cannot be asked.
    *r.host.list.lock().unwrap() = Err("Codespace is not connected".into());
    let wakes = r.host.ensures.load(Ordering::SeqCst);
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );
    assert_eq!(r.spy.deleted(), vec!["cs-1".to_string()]);
    assert_eq!(
        r.host.ensures.load(Ordering::SeqCst),
        wakes,
        "nothing was woken to check"
    );
}

#[test]
fn files_that_did_not_come_back_keep_the_instance_and_are_recorded() {
    let r = rig();
    let local = {
        let store = r.svc.store().unwrap();
        store
            .create("claude", r.w.project.to_str().unwrap())
            .unwrap()
            .id
    };
    let mut converted = hand_made(&r, local);
    converted.codespace = "cs-1".into();
    converted.ignored = vec![(12, "secrets.env".into()), (1, "node_modules/".into())];
    r.svc.arm_return_cleanup(
        ReturnSource {
            cloud: cs(),
            environment_id: "env-1".into(),
            host_session_id: Some("host-1".into()),
        },
        &mut converted,
    );
    assert!(r.record(local).is_none(), "not armed");
    assert!(
        converted
            .warnings
            .iter()
            .any(|w| w.contains("codespace cs-1 was kept") && w.contains("secrets.env")),
        "{:?}",
        converted.warnings
    );
    assert!(r
        .svc
        .unsaved_for_cloud(&cs())
        .iter()
        .any(|l| l.contains("secrets.env")));
    // A later run finds nothing to do; a manual delete still asks (the lines stay).
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Nothing
    );
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);

    // Files recorded after arming also stop the delete.
    let again = rig();
    let local = moved(&again.w);
    again.cont(local).unwrap();
    again
        .svc
        .record_unsaved_cloud(
            &cs(),
            &["Ignored files are not part of the local copy: a.env.".into()],
        )
        .unwrap();
    let CleanupOutcome::Kept { reason } = again.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("not on this computer"), "{reason}");
    assert_eq!(again.spy.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn moving_the_returned_session_to_the_cloud_again_starts_clean() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );
    let record = r
        .svc
        .begin_move(
            MoveRequest {
                session_id: local,
                target: MoveTarget::NewCodespace { machine: None },
                account_home: None,
                started_by: StartedBy::User,
                kit_off: false,
            },
            &|| false,
        )
        .expect("a new move starts");
    assert_eq!(record.session_id, local);
    let store = r.svc.store().unwrap();
    assert_eq!(store.cloud_location(local).unwrap(), SessionLocation::Local);
    assert!(store.remote_session(local).unwrap().is_none());
    assert!(store.cloud_session_ids(&cs()).unwrap().is_empty());
    assert_eq!(
        summary_provider(&store, local),
        Some(SESSION.to_string()),
        "the Claude id is kept"
    );
    drop(store);
    assert!(
        r.svc.sessions_on(&cs()).is_empty(),
        "the old instance is not offered"
    );
    assert!(!bound_to(&r, &cs()));
}

#[test]
fn an_ignored_file_the_return_left_behind_keeps_the_instance_in_a_real_return() {
    // The default fixture has an ignored `junk.log` in the workspace that no copy holds.
    let r = rig_with(world(true), own_only());
    let local = moved(&r.w);
    let done = r.cont(local).unwrap();
    assert!(r.record(local).is_none(), "not armed");
    assert!(
        done.warnings
            .iter()
            .any(|w| w.contains("codespace cs-1 was kept") && w.contains("junk.log")),
        "{:?}",
        done.warnings
    );
    assert!(r
        .svc
        .unsaved_for_cloud(&cs())
        .iter()
        .any(|l| l.contains("junk.log")));
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
}

fn arm_hand_made(r: &Rig, local: i64, cloud: &CloudRef, env: &str) -> Converted {
    let mut converted = hand_made(r, local);
    r.svc.arm_return_cleanup(
        ReturnSource {
            cloud: cloud.clone(),
            environment_id: env.into(),
            host_session_id: Some("host-1".into()),
        },
        &mut converted,
    );
    converted
}

fn new_session(r: &Rig) -> i64 {
    r.svc
        .store()
        .unwrap()
        .create("claude", r.w.project.to_str().unwrap())
        .unwrap()
        .id
}

#[test]
fn a_move_whose_target_is_this_instance_blocks_even_before_it_has_a_workspace() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let other = new_session(&r);
    let record = r
        .svc
        .begin_move(
            MoveRequest {
                session_id: other,
                target: MoveTarget::Existing {
                    backend: CloudBackendKind::Codespaces,
                    name: "cs-1".into(),
                },
                account_home: None,
                started_by: StartedBy::User,
                kit_off: false,
            },
            &|| false,
        )
        .unwrap();
    assert!(record.workspace.is_none(), "no workspace yet");
    let CleanupOutcome::Kept { reason } = r.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("move to it is not finished"), "{reason}");
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_start_in_progress_blocks() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    save_start_record(
        &r.svc.store().unwrap(),
        &StartRecord::fresh(&super::tests::resolved("cs-1"), 77, "/p", 1),
    )
    .unwrap();
    let CleanupOutcome::Kept { reason } = r.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("still starting"), "{reason}");
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_unreadable_location_blocks() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let other = new_session(&r);
    r.svc
        .store()
        .unwrap()
        .save_setting(&format!("cloud-location:{other}"), "{not json")
        .unwrap();
    let CleanupOutcome::Kept { reason } = r.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("running in it"), "{reason}");
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_recorded_clear_is_trusted_only_for_a_short_while() {
    // Inside the window: deletes without asking the unreachable Host.
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let created = r.record(local).unwrap().created_at;
    *r.host.list.lock().unwrap() = Err("Codespace is not connected".into());
    r.now
        .store(created + return_cleanup::CLEAR_TRUST_MS, Ordering::SeqCst);
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );

    // Past it: a retryable error, nothing woken, nothing deleted.
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let created = r.record(local).unwrap().created_at;
    *r.host.list.lock().unwrap() = Err("Codespace is not connected".into());
    r.now.store(
        created + return_cleanup::CLEAR_TRUST_MS + 1,
        Ordering::SeqCst,
    );
    let wakes = r.host.ensures.load(Ordering::SeqCst);
    let error = r.svc.run_return_cleanup("cs-1").unwrap_err();
    assert!(
        error.contains("must be running so Monoptah can check that no other session uses it"),
        "{error}"
    );
    assert_eq!(r.host.ensures.load(Ordering::SeqCst), wakes, "never woken");
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
    assert!(r.record(local).unwrap().needs_retry());
    // Running again, the live check answers and the retry deletes.
    *r.host.list.lock().unwrap() = own_only();
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::Deleted { warning: None }
    );
}

#[test]
fn new_work_on_the_instance_is_refused_while_it_is_being_deleted() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let other = new_session(&r);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let (weak, slot, project, other_id) = (
        r.weak(),
        seen.clone(),
        r.w.project.to_str().unwrap().to_string(),
        other,
    );
    r.spy.set_hook(move || {
        let svc = weak.upgrade().unwrap();
        let moved_in = svc.begin_move(
            MoveRequest {
                session_id: other_id,
                target: MoveTarget::Existing {
                    backend: CloudBackendKind::Codespaces,
                    name: "cs-1".into(),
                },
                account_home: None,
                started_by: StartedBy::User,
                kit_off: false,
            },
            &|| false,
        );
        let attached = svc.attach_session(&project, &CloudRef::codespace("cs-1"), HARNESS);
        let started = svc.begin_start(
            &project,
            Target {
                codespace: CodespaceChoice::Named("cs-1".into()),
                branch: None,
                machine: None,
                idle_timeout_minutes: None,
                devcontainer_path: None,
            },
            HARNESS,
        );
        let prepared = svc.prepare_target(
            &project,
            Target {
                codespace: CodespaceChoice::Named("cs-1".into()),
                branch: None,
                machine: None,
                idle_timeout_minutes: None,
                devcontainer_path: None,
            },
            &mut |_| {},
        );
        for result in [
            moved_in.map(|_| ()),
            attached.map(|_| ()),
            started.map(|_| ()),
            prepared.map(|_| ()),
        ] {
            slot.lock().unwrap().push(result.unwrap_err());
        }
    });
    r.svc.run_return_cleanup("cs-1").unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 4);
    for message in seen.iter() {
        assert!(message.contains("is being deleted"), "{message}");
    }
    // Nothing was recorded by the refused calls.
    assert!(pending_starts(&r.svc.store().unwrap()).is_empty());
}

#[test]
fn work_that_appears_just_before_the_delete_keeps_the_instance() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    // The Host listing already said nothing is there; then a start shows up.
    let weak = r.weak();
    r.spy.exists_hook.lock().unwrap().replace(Arc::new(move || {
        let svc = weak.upgrade().unwrap();
        save_start_record(
            &svc.store().unwrap(),
            &StartRecord::fresh(&super::tests::resolved("cs-1"), 88, "/p", 1),
        )
        .unwrap();
    }));
    let CleanupOutcome::Kept { reason } = r.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("still starting"), "{reason}");
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
    assert!(r.record(local).is_none());
}

#[test]
fn a_moved_claude_session_without_its_cloud_transcript_keeps_the_instance() {
    let r = rig();
    let local = moved(&r.w);
    // The cloud never had (or lost) Claude's own transcript file.
    let file =
        r.w.root
            .join("cs-home/.claude/projects")
            .join(claude_project_dir_name(r.w.ws.to_str().unwrap()))
            .join(format!("{SESSION}.jsonl"));
    std::fs::remove_file(file).unwrap();
    let done = r.cont(local).unwrap();
    assert!(done.transcript_lost);
    assert!(matches!(done.resume, Resume::Missing(_)));
    assert!(r.record(local).is_none(), "not armed");
    assert!(
        done.warnings
            .iter()
            .any(|w| w.contains("codespace cs-1 was kept")
                && w.contains("Claude transcript did not come back")),
        "{:?}",
        done.warnings
    );
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_session_that_never_ran_claude_still_arms_without_a_transcript() {
    let r = rig_with(world_with(false, false, false), own_only());
    let local = moved(&r.w);
    let done = r.cont(local).unwrap();
    assert!(!done.transcript_lost);
    assert!(r.record(local).is_some(), "{:?}", done.warnings);
}

#[test]
fn an_instance_that_is_already_gone_needs_no_question_to_the_host() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let created = r.record(local).unwrap().created_at;
    // The recorded answer is stale, the Host cannot be asked, and the backend says it is gone.
    r.now.store(
        created + return_cleanup::CLEAR_TRUST_MS + 1,
        Ordering::SeqCst,
    );
    *r.host.list.lock().unwrap() = Err("Codespace is not connected".into());
    r.spy.exists.store(false, Ordering::SeqCst);
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::AlreadyGone
    );
    assert!(r.record(local).is_none());
    assert!(!bound_to(&r, &cs()), "its bindings are forgotten");
}

#[test]
fn a_gone_instance_that_sessions_still_refer_to_is_not_called_running() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let other = new_session(&r);
    r.svc
        .store()
        .unwrap()
        .save_setting(&format!("cloud-location:{other}"), "{not json")
        .unwrap();
    r.spy.exists.store(false, Ordering::SeqCst);
    let CleanupOutcome::Kept { reason } = r.svc.run_return_cleanup("cs-1").unwrap() else {
        panic!("expected Kept");
    };
    assert!(reason.contains("no longer exists"), "{reason}");
    assert!(
        !reason.contains("left running") && !reason.contains("keeps running"),
        "{reason}"
    );
}

#[test]
fn a_second_return_that_finds_the_instance_being_deleted_saves_a_retry() {
    let r = rig();
    let local = moved(&r.w);
    r.svc.cleaning.lock().unwrap().insert("cs-1".into());
    let done = r.cont(local).unwrap();
    r.svc.cleaning.lock().unwrap().clear();
    let record = r.record(local).expect("the decision is not lost");
    assert!(record.needs_retry() && !record.auto_run());
    assert!(
        record
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("another return was deciding")),
        "{record:?}"
    );
    assert!(
        done.warnings.iter().any(|w| w.contains("deciding")),
        "{:?}",
        done.warnings
    );
}

#[test]
fn a_delete_that_worked_but_could_not_be_recorded_is_reported_as_deleted_and_retryable() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    // The backend deleted it but then failed; the instance is confirmed gone afterwards.
    let spy = r.spy.clone();
    r.spy
        .set_hook(move || spy.exists.store(false, Ordering::SeqCst));
    *r.spy.fail_next.lock().unwrap() = Some("could not save".into());
    let outcome = r.svc.run_return_cleanup("cs-1").unwrap();
    let CleanupOutcome::Deleted { warning: Some(w) } = outcome else {
        panic!("expected Deleted with a warning, got {outcome:?}");
    };
    assert!(
        w.contains("could not save") && w.contains("choose Retry"),
        "{w}"
    );
    assert!(r.record(local).unwrap().needs_retry(), "kept for a retry");
    assert!(r.record(local).unwrap().gone, "marked as gone");
    // The retry finds it gone.
    assert_eq!(
        r.svc.run_return_cleanup("cs-1").unwrap(),
        CleanupOutcome::AlreadyGone
    );
    assert!(r.record(local).is_none());
}

#[test]
fn a_missing_backend_keeps_the_record_with_a_message() {
    let r = rig();
    // The fixture's own service reads the real clock.
    r.now.store(crate::time::now_ms(), Ordering::SeqCst);
    let local = new_session(&r);
    arm_hand_made(&r, local, &CloudRef::sandbox("sbx-1"), "env-sbx");
    // The fixture's own service has no sandbox backend.
    let error = r.w.svc.run_return_cleanup("sbx:sbx-1").unwrap_err();
    assert!(error.contains("backend"), "{error}");
    let record = r.record(local).unwrap();
    assert_eq!(record.last_error.as_deref(), Some(error.as_str()));
}

#[test]
fn a_session_with_records_for_two_instances_is_told_apart_by_instance() {
    let r = rig();
    let local = new_session(&r);
    arm_hand_made(&r, local, &CloudRef::sandbox("sbx-1"), "env-sbx");
    r.now.fetch_add(10, Ordering::SeqCst);
    arm_hand_made(&r, local, &cs(), "env-1");
    assert_eq!(r.svc.return_cleanups_for_session(local).len(), 2);
    assert_eq!(r.svc.return_cleanup_for(local, &cs()).unwrap().cloud, cs());
    assert_eq!(
        r.svc
            .return_cleanup_for(local, &CloudRef::sandbox("sbx-1"))
            .unwrap()
            .key(),
        "sbx:sbx-1"
    );
    assert!(r
        .svc
        .return_cleanup_for(local, &CloudRef::codespace("zz"))
        .is_none());
    // The menu record is the newest one that needs a Retry.
    r.svc.defer_return_cleanup("sbx:sbx-1", "busy");
    assert_eq!(
        r.svc.return_cleanup_for_session(local).unwrap().key(),
        "sbx:sbx-1"
    );
    r.svc.defer_return_cleanup("cs-1", "busy");
    assert_eq!(
        r.svc.return_cleanup_for_session(local).unwrap().key(),
        "cs-1"
    );
}

#[test]
fn a_kept_instance_names_a_path_that_exists() {
    let r = rig();
    r.svc
        .store()
        .unwrap()
        .forget_cloud_session(r.w.cloud_id)
        .unwrap();
    // Nothing in the app refers to it: say plainly that it must be deleted by hand.
    let hint = r.svc.kept_instance_hint(&CloudRef::sandbox("sbx-1"), None);
    assert!(
        hint.contains("no longer lists it") && hint.contains("sandbox sbx-1"),
        "{hint}"
    );
    assert!(!hint.contains("sbx rm") && !hint.contains("--"), "{hint}");
    let hint = r.svc.kept_instance_hint(&cs(), None);
    assert!(hint.contains("gh codespace delete -c cs-1"), "{hint}");
    // Sessions still use it: their menus are the path.
    let r = rig();
    let hint = r.svc.kept_instance_hint(&cs(), None);
    assert!(hint.contains("archive dialog"), "{hint}");
}

#[test]
fn the_hint_points_at_the_session_menu_when_the_returned_session_has_an_origin() {
    let r = rig();
    let done = r.cont(r.w.cloud_id).unwrap();
    let hint = r.svc.kept_instance_hint(&cs(), Some(done.local_id));
    assert!(hint.contains("Delete codespace cs-1"), "{hint}");
    assert!(!hint.contains("gh codespace"), "{hint}");
}

fn stored_key_set(r: &Rig, record: &ReturnCleanup) {
    r.svc
        .store()
        .unwrap()
        .save_setting(
            &format!("cloud-return-cleanup:{}", record.key()),
            &serde_json::to_string(record).unwrap(),
        )
        .unwrap();
}

#[test]
fn a_gone_record_that_cannot_be_confirmed_offline_asks_for_another_try() {
    let r = rig();
    let local = moved(&r.w);
    r.cont(local).unwrap();
    let mut record = r.record(local).unwrap();
    record.gone = true;
    stored_key_set(&r, &record);
    *r.spy.exists_error.lock().unwrap() = Some("offline".into());
    let error = r.svc.run_return_cleanup("cs-1").unwrap_err();
    assert!(
        error.contains("could not confirm") && error.contains("Finish cleanup again"),
        "{error}"
    );
    assert!(!error.contains("start it"), "{error}");
    assert!(r.record(local).unwrap().needs_retry());
    assert_eq!(r.spy.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_older_run_never_overwrites_the_record_of_a_newer_return() {
    for fail_delete in [false, true] {
        let r = rig();
        let local = moved(&r.w);
        r.cont(local).unwrap();
        let first = r.record(local).unwrap();
        // While this run works, another return of the same instance stores its own record.
        let mut newer = first.clone();
        newer.local_id = local + 100;
        newer.created_at += 5;
        let (weak, newer_copy) = (r.weak(), newer.clone());
        r.spy.exists_hook.lock().unwrap().replace(Arc::new(move || {
            let svc = weak.upgrade().unwrap();
            svc.store()
                .unwrap()
                .save_setting(
                    &format!("cloud-return-cleanup:{}", newer_copy.key()),
                    &serde_json::to_string(&newer_copy).unwrap(),
                )
                .unwrap();
        }));
        if fail_delete {
            *r.spy.fail_next.lock().unwrap() = Some("gh is offline".into());
            assert!(r.svc.run_return_cleanup("cs-1").is_err());
        } else {
            r.svc.run_return_cleanup("cs-1").unwrap();
        }
        assert_eq!(
            r.svc.return_cleanups(),
            vec![newer],
            "the newer record is untouched (fail_delete={fail_delete})"
        );
    }
}

#[test]
fn a_start_is_refused_under_the_store_lock_while_the_instance_is_being_deleted() {
    let r = rig();
    r.svc.cleaning.lock().unwrap().insert("cs-1".into());
    let error = r
        .svc
        .record_start("/p", HARNESS, &super::tests::resolved("cs-1"), 1, false)
        .unwrap_err();
    assert!(error.contains("is being deleted"), "{error}");
    assert!(pending_starts(&r.svc.store().unwrap()).is_empty());
}
