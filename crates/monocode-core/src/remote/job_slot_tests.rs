//! The SSH setup slot: jobs for different cloud targets run side by side, jobs that would
//! share a target or an interactive resource are serialized, and a finished job stays
//! readable for its owner.
#![cfg(unix)]

use super::ssh::{self, GhOverride, JobTarget};
use super::*;
use crate::cloud_sessions::{Host, START_ABORTED};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

/// A fake `gh` that runs for `seconds` and then fails, so a job stays active for a while and
/// then ends with an error (the tests only look at how jobs are admitted and kept).
fn fake_gh(dir: &Path, seconds: u32) -> GhOverride {
    let gh = dir.join("gh");
    std::fs::write(
        &gh,
        format!("#!/bin/sh\nif [ {seconds} -gt 0 ]; then exec sleep {seconds}; fi\nexit 1\n"),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    GhOverride::set(gh)
}

/// A fake home in which `gh` has already created its automatic key pair.
fn home_with_key(dir: &Path) -> PathBuf {
    let home = dir.join("home-with-key");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/codespaces.auto"), "key").unwrap();
    std::fs::write(home.join(".ssh/codespaces.auto.pub"), "key.pub").unwrap();
    home
}

fn empty_home(dir: &Path) -> PathBuf {
    let home = dir.join("home-without-key");
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn wait_done(connections: &Connections, job: &str) -> JobView {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let view = connections.ssh_poll(job).unwrap();
        if view.done {
            return view;
        }
        assert!(Instant::now() < deadline, "job did not finish: {view:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn hold(connections: &Connections, target: JobTarget) -> Arc<Job> {
    let job = Job::for_target(target);
    connections
        .jobs
        .lock()
        .unwrap()
        .insert(job.view().id, job.clone());
    job
}

#[test]
fn codespaces_with_different_names_are_set_up_at_the_same_time() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 1);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let first = connections.codespace_begin("octo-aaa", None).unwrap();
    let second = connections.codespace_begin("octo-bbb", None).unwrap();
    assert!(!connections.ssh_poll(&first).unwrap().done);
    wait_done(&connections, &first);
    wait_done(&connections, &second);
}

#[test]
fn the_same_codespace_cannot_be_set_up_twice_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 1);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let first = connections.codespace_begin("octo-aaa", None).unwrap();
    assert_eq!(
        connections.codespace_begin("octo-aaa", None).unwrap_err(),
        SSH_BUSY
    );
    wait_done(&connections, &first);
    assert!(connections.codespace_begin("octo-aaa", None).is_ok());
}

#[test]
fn a_plain_ssh_job_blocks_a_codespace_job_and_the_other_way_round() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 1);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let plain = hold(&connections, JobTarget::Ssh("me@box".into(), None));
    assert_eq!(
        connections.codespace_begin("octo-aaa", None).unwrap_err(),
        SSH_BUSY
    );
    plain.complete(|| Err("done".into()));
    let codespace = connections.codespace_begin("octo-aaa", None).unwrap();
    assert_eq!(
        connections
            .ssh_begin("me@box".into(), "box".into(), None)
            .unwrap_err(),
        SSH_BUSY
    );
    wait_done(&connections, &codespace);
}

#[test]
fn a_finished_job_stays_readable_after_another_job_starts() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 0);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let first = connections.codespace_begin("octo-aaa", None).unwrap();
    wait_done(&connections, &first);
    let second = connections.codespace_begin("octo-bbb", None).unwrap();
    let view = connections.ssh_poll(&first).expect("first job expired");
    assert!(view.done);
    wait_done(&connections, &second);
    assert!(connections.ssh_poll(&first).is_ok());
}

#[test]
fn finished_jobs_are_dropped_only_after_the_retention_time() {
    let connections = Connections::new(tempfile::tempdir().unwrap().path());
    let job = hold(&connections, JobTarget::Codespace("octo-aaa".into()));
    job.complete(|| Err("done".into()));
    let running = hold(&connections, JobTarget::Codespace("octo-bbb".into()));
    let mut jobs = connections.jobs.lock().unwrap();
    prune_finished_jobs_at(&mut jobs, Instant::now() + Duration::from_secs(60));
    assert_eq!(jobs.len(), 2);
    prune_finished_jobs_at(&mut jobs, Instant::now() + Duration::from_secs(60 * 60));
    assert_eq!(jobs.len(), 1);
    assert!(jobs.contains_key(&running.view().id));
}

#[test]
fn codespace_jobs_are_serialized_until_gh_has_created_its_key() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 1);
    let home = empty_home(dir.path());
    gh.set_home(home.clone());
    let connections = Arc::new(Connections::new(dir.path()));
    let first = connections.codespace_begin("octo-aaa", None).unwrap();
    assert_eq!(
        connections.codespace_begin("octo-bbb", None).unwrap_err(),
        SSH_BUSY
    );
    wait_done(&connections, &first);
    // The first job is done and the key still does not exist: the next one may start.
    let second = connections.codespace_begin("octo-bbb", None).unwrap();
    // Once the key exists, codespaces no longer wait for each other.
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/codespaces.auto"), "key").unwrap();
    std::fs::write(home.join(".ssh/codespaces.auto.pub"), "key.pub").unwrap();
    assert!(connections.codespace_begin("octo-ccc", None).is_ok());
    wait_done(&connections, &second);
}

#[test]
fn connect_waits_for_the_slot_instead_of_failing_busy() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 0);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let held = hold(&connections, JobTarget::Codespace("octo-aaa".into()));
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = {
        let connections = connections.clone();
        std::thread::spawn(move || {
            let halt = AtomicBool::new(false);
            let result = Host::connect(&connections, "octo-aaa", "Cloud", &halt, &mut |_| {});
            sender.send(result.map(|m| m.id)).unwrap();
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    assert!(receiver.try_recv().is_err(), "connect gave up while busy");
    held.complete(|| Err("done".into()));
    let result = receiver.recv_timeout(Duration::from_secs(20)).unwrap();
    // The fake gh fails, but the start got a slot and ran.
    assert_ne!(result.unwrap_err(), SSH_BUSY);
    worker.join().unwrap();
}

#[test]
fn connect_stops_waiting_for_the_slot_when_halted() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 0);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let _held = hold(&connections, JobTarget::Codespace("octo-aaa".into()));
    let halt = Arc::new(AtomicBool::new(false));
    let worker = {
        let (connections, halt) = (connections.clone(), halt.clone());
        std::thread::spawn(move || {
            let halt = halt;
            Host::connect(&connections, "octo-aaa", "Cloud", &halt, &mut |_| {}).map(|m| m.id)
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    halt.store(true, Ordering::Relaxed);
    let started = Instant::now();
    assert_eq!(worker.join().unwrap().unwrap_err(), START_ABORTED);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn connect_cancels_its_running_job_when_halted() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 20);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let halt = Arc::new(AtomicBool::new(false));
    let worker = {
        let (connections, halt) = (connections.clone(), halt.clone());
        std::thread::spawn(move || {
            let halt = halt;
            Host::connect(&connections, "octo-aaa", "Cloud", &halt, &mut |_| {}).map(|m| m.id)
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    halt.store(true, Ordering::Relaxed);
    let started = Instant::now();
    assert_eq!(worker.join().unwrap().unwrap_err(), START_ABORTED);
    assert!(started.elapsed() < Duration::from_secs(10));
    let jobs = connections.jobs.lock().unwrap();
    assert!(jobs
        .values()
        .all(|job| job.cancelled.load(Ordering::Relaxed)));
}

#[test]
fn sandboxes_run_beside_other_sandboxes_and_codespaces_but_not_beside_themselves() {
    // Decided on the rule itself: starting a sandbox job would run the real ssh.
    let sbx = |name: &str| JobTarget::Sandbox(name.into());
    let cs = |name: &str| JobTarget::Codespace(name.into());
    let ssh = JobTarget::Ssh("me@box".into(), None);
    for key_ready in [true, false] {
        assert!(ssh::jobs_conflict(
            Some(&sbx("a")),
            Some(&sbx("a")),
            key_ready
        ));
        assert!(!ssh::jobs_conflict(
            Some(&sbx("a")),
            Some(&sbx("b")),
            key_ready
        ));
        assert!(!ssh::jobs_conflict(
            Some(&sbx("a")),
            Some(&cs("a")),
            key_ready
        ));
        assert!(!ssh::jobs_conflict(
            Some(&cs("a")),
            Some(&sbx("a")),
            key_ready
        ));
        assert!(ssh::jobs_conflict(Some(&sbx("a")), Some(&ssh), key_ready));
        assert!(ssh::jobs_conflict(Some(&ssh), Some(&sbx("a")), key_ready));
        assert!(ssh::jobs_conflict(None, Some(&sbx("a")), key_ready));
        assert!(ssh::jobs_conflict(Some(&sbx("a")), None, key_ready));
    }
    assert!(!ssh::jobs_conflict(Some(&cs("a")), Some(&cs("b")), true));
    assert!(ssh::jobs_conflict(Some(&cs("a")), Some(&cs("b")), false));
}

#[test]
fn a_sandbox_job_in_the_map_does_not_block_a_codespace_job() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 0);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let _sandbox = hold(&connections, JobTarget::Sandbox("box".into()));
    let job = connections.codespace_begin("octo-aaa", None).unwrap();
    wait_done(&connections, &job);
}

#[test]
fn an_upgrade_slot_waits_only_for_jobs_on_its_own_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 0);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    let x = || Some(JobTarget::Codespace("octo-xxx".into()));
    let never = AtomicBool::new(false);
    // A setup job for codespace Y runs: the upgrade of X is granted at once.
    let other = hold(&connections, JobTarget::Codespace("octo-yyy".into()));
    let mut told = false;
    let (_, slot) = connections
        .hold_job_slot(x(), &never, &mut || told = true)
        .unwrap();
    assert!(!told, "granted without waiting");
    // While X is upgraded, a setup job for X is refused and one for Y is not.
    assert_eq!(
        connections.codespace_begin("octo-xxx", None).unwrap_err(),
        SSH_BUSY
    );
    other.complete(|| Err("done".into()));
    let again = connections.codespace_begin("octo-yyy", None).unwrap();
    wait_done(&connections, &again);
    drop(slot);
    // A setup job for X runs: the upgrade of X waits and stops when cancelled.
    let _setup = hold(&connections, JobTarget::Codespace("octo-xxx".into()));
    let cancel = Arc::new(AtomicBool::new(false));
    let flip = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        flip.store(true, Ordering::Relaxed);
    });
    let mut told = false;
    let error = connections
        .hold_job_slot(x(), &cancel, &mut || told = true)
        .err()
        .unwrap();
    canceller.join().unwrap();
    assert_eq!(error, WAKE_CANCELLED);
    assert!(told, "said it was waiting");
}

#[test]
fn a_start_that_waited_for_the_slot_wakes_the_machine_the_other_start_paired() {
    let dir = tempfile::tempdir().unwrap();
    // Written only once the start is waiting: the other start pairs it meanwhile.
    let machines_json = format!(
        r#"[{{"id":"m1","name":"Cloud","endpoint":"codespace://octo-aaa","environmentId":"env","token":"{}","ssh":{{"target":"octo-aaa","remotePort":3774,"codespace":"octo-aaa"}}}}]"#,
        "t".repeat(43)
    );
    let gh = fake_gh(dir.path(), 0);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    // The job this start waits for is still pairing the same codespace.
    let held = hold(&connections, JobTarget::Codespace("octo-aaa".into()));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let worker = {
        let (connections, lines) = (connections.clone(), lines.clone());
        std::thread::spawn(move || {
            let halt = AtomicBool::new(false);
            Host::connect(&connections, "octo-aaa", "Cloud", &halt, &mut |line| {
                lines.lock().unwrap().push(line.to_string())
            })
        })
    };
    std::thread::sleep(Duration::from_millis(400));
    assert!(!worker.is_finished(), "connect did not wait for the slot");
    std::fs::write(dir.path().join("remote-machines.json"), machines_json).unwrap();
    held.complete(|| Err("done".into()));
    // The fake gh fails the wake, but it is the wake of the paired machine that ran.
    assert!(worker.join().unwrap().is_err());
    let lines = lines.lock().unwrap();
    assert!(lines.iter().any(|l| l.starts_with("Waking")), "{lines:?}");
    let machines = connections.list().unwrap();
    assert_eq!(machines.len(), 1);
    assert_eq!(machines[0].id, "m1");
}

#[test]
fn the_start_path_waits_for_a_target_a_job_has_just_paired_but_the_ui_path_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), 0);
    gh.set_home(home_with_key(dir.path()));
    let connections = Arc::new(Connections::new(dir.path()));
    // A job for X finished with a machine; nothing is running.
    let done = hold(&connections, JobTarget::Codespace("octo-xxx".into()));
    done.complete(|| {
        Ok(Machine {
            id: "m1".into(),
            name: "Cloud".into(),
            endpoint: "codespace://octo-xxx".into(),
            environment_id: "env".into(),
            ssh: None,
            codespace: Some("octo-xxx".into()),
            sandbox: None,
        })
    });
    assert_eq!(
        connections
            .codespace_begin_for_start("octo-xxx", None)
            .unwrap_err(),
        SSH_BUSY
    );
    let other = connections
        .codespace_begin_for_start("octo-yyy", None)
        .unwrap();
    let ui = connections.codespace_begin("octo-xxx", None).unwrap();
    wait_done(&connections, &other);
    wait_done(&connections, &ui);
}
