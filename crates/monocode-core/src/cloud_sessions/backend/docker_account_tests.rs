//! The Docker account of each sandbox in `DockerBackend` and `SystemShell`.
use super::docker::*;
use super::CloudBackend;
use crate::docker_accounts::test_support::Installed;
use crate::remote::SandboxTarget;
use crate::sandboxes::test_support::AccountSbx;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn resolver(pins: &[(&str, &str)]) -> (AccountResolver, Arc<AtomicUsize>) {
    let map: HashMap<String, String> = pins
        .iter()
        .map(|(n, a)| (n.to_string(), a.to_string()))
        .collect();
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = reads.clone();
    (
        Arc::new(move |name| {
            counted.fetch_add(1, Ordering::SeqCst);
            map.get(name).cloned()
        }),
        reads,
    )
}

fn sandboxes() -> Arc<AccountSbx> {
    let sbx = Arc::new(AccountSbx::new());
    sbx.add("sbx_001a", "box-a", "running", Some("A"));
    sbx.add("sbx_002b", "box-b", "running", Some("B"));
    sbx.add("sbx_003m", "box-m", "running", None);
    sbx
}

#[test]
fn every_lifecycle_call_runs_as_the_account_of_that_sandbox() {
    let sbx = sandboxes();
    let (resolve, _) = resolver(&[("box-a", "A"), ("box-b", "B")]);
    let backend = DockerBackend::new(sbx.clone()).with_account_resolver(resolve);
    for name in ["box-a", "box-b", "box-m"] {
        assert_eq!(backend.state(name).unwrap(), "running", "{name}");
        assert!(backend.exists(name).unwrap());
    }
    backend.stop("box-a").unwrap();
    backend.stop("box-b").unwrap();
    backend.delete("box-a").unwrap();
    backend.delete("box-m").unwrap();
    let calls = sbx.calls();
    // Name lookups (ls), state reads (inspect), stop and rm: each as the sandbox's account.
    let accounts_for = |verb: &str| sbx.accounts_of(verb);
    assert!(accounts_for("ls").contains(&Some("A".into())));
    assert!(accounts_for("ls").contains(&Some("B".into())));
    assert!(accounts_for("ls").contains(&None));
    assert_eq!(accounts_for("stop"), [Some("A".into()), Some("B".into())]);
    assert_eq!(accounts_for("rm"), [Some("A".into()), None]);
    // A is never used for box-b's calls or the machine login for box-a's: the fake would
    // have answered "not found", and every call above succeeded.
    assert!(!calls.is_empty());
    assert!(!sbx.exists("sbx_001a") && sbx.exists("sbx_002b") && !sbx.exists("sbx_003m"));
}

#[test]
fn a_sandbox_in_an_account_looks_missing_to_the_machine_login() {
    let sbx = sandboxes();
    let (none, _) = resolver(&[]);
    let backend = DockerBackend::new(sbx).with_account_resolver(none);
    // Without a pin the backend asks as the machine login, which cannot see box-a. This is
    // why the pin matters: the caller must never read this as "deleted" for a pinned one.
    assert_eq!(backend.exists("box-a"), Ok(false));
}

#[test]
fn a_known_account_is_remembered_and_a_missing_one_is_looked_up_again() {
    let sbx = sandboxes();
    let late: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let reads = Arc::new(AtomicUsize::new(0));
    let (source, counted) = (late.clone(), reads.clone());
    let backend = DockerBackend::new(sbx).with_account_resolver(Arc::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        source.lock().unwrap().clone()
    }));
    // No pin yet: not cached, so a pin written a moment later (a create that just
    // succeeded) is seen.
    assert_eq!(backend.account_of("box-a"), None);
    assert_eq!(backend.account_of("box-a"), None);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    *late.lock().unwrap() = Some("A".into());
    assert_eq!(backend.account_of("box-a").as_deref(), Some("A"));
    // Now remembered: the store is not read again.
    *late.lock().unwrap() = None;
    assert_eq!(backend.account_of("box-a").as_deref(), Some("A"));
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    // Told explicitly, it wins; told nothing, it forgets.
    backend.remember_account("box-a", Some("B"));
    assert_eq!(backend.account_of("box-a").as_deref(), Some("B"));
    backend.remember_account("box-a", None);
    assert_eq!(backend.account_of("box-a"), None);
}

#[test]
fn deleting_a_sandbox_forgets_its_account() {
    let sbx = sandboxes();
    let (resolve, reads) = resolver(&[("box-a", "A")]);
    let backend = DockerBackend::new(sbx).with_account_resolver(resolve);
    backend.delete("box-a").unwrap();
    let before = reads.load(Ordering::SeqCst);
    assert_eq!(backend.account_of("box-a").as_deref(), Some("A"));
    assert_eq!(
        reads.load(Ordering::SeqCst),
        before + 1,
        "read again from the pin setting, not kept in memory"
    );
}

#[test]
fn a_remembered_target_brings_its_account() {
    let sbx = sandboxes();
    let backend = DockerBackend::new(sbx);
    backend.remember_target(&SandboxTarget {
        id: "sbx_002b".into(),
        name: "box-b".into(),
        workspace: "/home/agent/workspaces/ws".into(),
        relay_port: 47200,
        config: None,
        mode: Default::default(),
        docker_account: Some("B".into()),
    });
    assert_eq!(backend.account_of("box-b").as_deref(), Some("B"));
    assert_eq!(backend.state("box-b").unwrap(), "running");
}

// ------------------------------------------------------------------- the ssh seam

#[derive(Default)]
struct RecordingShell {
    accounts: Mutex<Vec<(String, Option<String>)>>,
}
impl SandboxShell for RecordingShell {
    fn run(
        &self,
        id: &str,
        remote: &str,
        stdin: &str,
        on_line: &mut dyn FnMut(&str),
        abort: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<ShellOutput, String> {
        self.run_as(None, id, remote, stdin, on_line, abort, timeout)
    }
    fn run_as(
        &self,
        account: Option<&str>,
        id: &str,
        _remote: &str,
        _stdin: &str,
        _on_line: &mut dyn FnMut(&str),
        _abort: &dyn Fn() -> bool,
        _timeout: Duration,
    ) -> Result<ShellOutput, String> {
        self.accounts
            .lock()
            .unwrap()
            .push((id.into(), account.map(str::to_owned)));
        Ok(ShellOutput {
            success: true,
            stdout: "ok\n".into(),
            stderr: String::new(),
        })
    }
}

fn target(name: &str, id: &str, account: Option<&str>) -> SandboxTarget {
    SandboxTarget {
        id: id.into(),
        name: name.into(),
        workspace: "/home/agent/workspaces/ws".into(),
        relay_port: 47200,
        config: None,
        mode: Default::default(),
        docker_account: account.map(str::to_owned),
    }
}

#[test]
fn scripts_copies_and_outer_scripts_run_as_the_sandboxs_account() {
    let shell = Arc::new(RecordingShell::default());
    let (resolve, _) = resolver(&[("box-out", "C")]);
    let backend = DockerBackend::new(Arc::new(AccountSbx::new()))
        .with_shell(shell.clone())
        .with_account_resolver(resolve);
    backend.remember_target(&target("box-a", "sbx_001a", Some("A")));
    backend.remember_target(&target("box-m", "sbx_003m", None));
    backend.remember("box-out", "sbx_004o");
    backend.run_script("box-a", "echo", false).unwrap();
    backend.run_script("box-m", "echo", false).unwrap();
    // Outer scripts need only the id; their account comes from the pin.
    backend
        .run_outer_script("box-out", "/home/agent/workspaces/ws", "echo", false)
        .unwrap();
    assert_eq!(
        *shell.accounts.lock().unwrap(),
        [
            ("sbx_001a".to_string(), Some("A".to_string())),
            ("sbx_003m".to_string(), None),
            ("sbx_004o".to_string(), Some("C".to_string())),
        ]
    );
}

// --------------------------------------------- the real ssh process (a script as `ssh`)

#[cfg(unix)]
fn fake_ssh(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("ssh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn ran(shell: &SystemShell, account: Option<&str>) -> Result<ShellOutput, String> {
    shell.run_as(
        account,
        "sbx_001abc",
        "sh -s",
        "",
        &mut |_| {},
        &|| false,
        Duration::from_secs(20),
    )
}

#[cfg(unix)]
#[test]
fn the_real_ssh_gets_the_token_in_its_environment_only_for_an_account() {
    let installed = Installed::new();
    let id = installed.add("alice");
    let dir = tempfile::tempdir().unwrap();
    // Reports whether the variable is set and whether any argument holds a JWT.
    let ssh = fake_ssh(
        dir.path(),
        r#"if [ -n "$DOCKER_ACCESS_TOKEN" ]; then echo set; else echo unset; fi
case "$*" in *eyJ*) echo argv-leak;; esac"#,
    );
    let shell = SystemShell::with_program(ssh);
    let with = ran(&shell, Some(&id)).unwrap();
    assert_eq!(with.stdout.trim(), "set", "{with:?}");
    assert!(!with.stdout.contains("argv-leak"));
    let calls = installed.hub.calls();
    // Another process, another read of the cache (still the same JWT while it is valid).
    ran(&shell, Some(&id)).unwrap();
    assert_eq!(installed.hub.calls(), calls);
    for jwt in installed.hub.issued() {
        assert!(!with.stdout.contains(&jwt) && !with.stderr.contains(&jwt));
    }
}

#[cfg(unix)]
#[test]
fn the_real_ssh_without_an_account_gets_no_token() {
    let installed = Installed::new();
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(
        dir.path(),
        r#"if [ -n "$DOCKER_ACCESS_TOKEN" ]; then echo set; else echo unset; fi"#,
    );
    let shell = SystemShell::with_program(ssh);
    // The plain `run` is the machine login.
    let out = shell
        .run(
            "sbx_001abc",
            "sh -s",
            "",
            &mut |_| {},
            &|| false,
            Duration::from_secs(20),
        )
        .unwrap();
    // Whatever the test runner inherited, the shell added nothing of its own.
    let inherited = std::env::var_os("DOCKER_ACCESS_TOKEN").is_some();
    if !inherited {
        assert_eq!(out.stdout.trim(), "unset");
    }
    assert_eq!(installed.hub.calls(), 0);
}

#[cfg(unix)]
#[test]
fn an_unusable_account_never_starts_ssh() {
    let installed = Installed::new();
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let ssh = fake_ssh(dir.path(), &format!("touch '{}'", marker.display()));
    let shell = SystemShell::with_program(ssh);
    let error = ran(&shell, Some("removed-account")).unwrap_err();
    assert!(error.contains("no longer exists"), "{error}");
    assert!(
        !marker.exists(),
        "ssh must not run as the machine login instead"
    );
    drop(installed);
}

#[cfg(unix)]
#[test]
fn a_rejection_from_the_proxy_command_drops_the_cached_token() {
    let installed = Installed::new();
    let id = installed.add("alice");
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(
        dir.path(),
        "echo 'error: DOCKER_ACCESS_TOKEN was rejected' >&2\nexit 255",
    );
    let shell = SystemShell::with_program(ssh);
    let out = ran(&shell, Some(&id)).unwrap();
    assert!(!out.success);
    let after_first = installed.hub.calls();
    // The next ssh exchanges again instead of reusing the refused JWT.
    let _ = ran(&shell, Some(&id));
    assert_eq!(installed.hub.calls(), after_first + 1);
}
