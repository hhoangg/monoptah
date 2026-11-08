use super::docker::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(unix)]
fn fake_ssh(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("ssh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn the_shell_runs_ssh_to_the_alias_with_the_script_on_stdin_and_streams_lines() {
    let dir = tempfile::tempdir().unwrap();
    let args = dir.path().join("args");
    let ssh = fake_ssh(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > {}\ncat\necho oops >&2\necho tail",
            args.display()
        ),
    );
    let shell = SystemShell::with_program(ssh);
    let mut lines = vec![];
    let out = shell
        .run(
            "sbx_001abc",
            "sh -s",
            "echo from-stdin\nsecond line\n",
            &mut |l| lines.push(l.to_string()),
            &|| false,
            Duration::from_secs(20),
        )
        .unwrap();
    assert!(out.success);
    assert_eq!(out.stdout, "echo from-stdin\nsecond line\ntail\n");
    assert_eq!(out.stderr, "oops\n");
    assert!(lines.contains(&"second line".to_string()));
    let argv = std::fs::read_to_string(&args).unwrap();
    let argv: Vec<&str> = argv.lines().collect();
    assert!(argv.contains(&"BatchMode=yes"));
    assert!(argv.contains(&"ConnectTimeout=120"));
    // The destination and the one remote word come last, after `--`.
    let n = argv.len();
    assert_eq!(argv[n - 3], "--");
    assert_eq!(argv[n - 2], "sbx_001abc@sbx_cloud");
    assert_eq!(argv[n - 1], "sh -s");
    assert!(
        !argv.iter().any(|a| a.contains("from-stdin")),
        "stdin never reaches argv"
    );
}

#[cfg(unix)]
#[test]
fn a_failing_command_is_ok_with_success_false_and_a_bad_id_never_spawns() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "echo denied >&2\nexit 255");
    let shell = SystemShell::with_program(ssh);
    let out = shell
        .run(
            "sbx_1",
            "true",
            "",
            &mut |_| {},
            &|| false,
            Duration::from_secs(20),
        )
        .unwrap();
    assert!(!out.success);
    assert_eq!(out.stderr, "denied\n");
    assert!(shell
        .run(
            "-oProxyCommand=x",
            "true",
            "",
            &mut |_| {},
            &|| false,
            Duration::from_secs(5)
        )
        .is_err());
}

#[cfg(unix)]
#[test]
fn abort_kills_a_running_command() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "echo started\nsleep 30");
    let shell = SystemShell::with_program(ssh);
    let stop = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let error = shell
        .run(
            "sbx_1",
            "true",
            "",
            &mut |_| stop.store(true, Ordering::SeqCst),
            &|| stop.load(Ordering::SeqCst),
            Duration::from_secs(60),
        )
        .unwrap_err();
    assert_eq!(error, SHELL_ABORTED);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[cfg(unix)]
#[test]
fn secrets_travel_on_stdin_and_never_in_argv() {
    const SECRET: &str = "ghp_SECRETSECRET123456";
    let dir = tempfile::tempdir().unwrap();
    let args = dir.path().join("args");
    let ssh = fake_ssh(
        dir.path(),
        &format!("printf '%s\\n' \"$@\" > {}\ncat >/dev/null", args.display()),
    );
    let shell = SystemShell::with_program(ssh);
    // The approve call (script in argv, payload on stdin) and the bootstrap call (script and
    // prelude on stdin, fixed exec command in argv).
    for (remote, stdin) in [
        (
            "sh -c 'git credential approve'",
            format!("password={SECRET}\n"),
        ),
        (
            "$HOME/.devcontainers/bin/devcontainer exec --workspace-folder '/w' -- sh -l -s",
            format!("export CLAUDE_CODE_OAUTH_TOKEN='{SECRET}'\n"),
        ),
    ] {
        let out = shell
            .run(
                "sbx_1",
                remote,
                &stdin,
                &mut |_| {},
                &|| false,
                Duration::from_secs(20),
            )
            .unwrap();
        assert!(out.success);
        let argv = std::fs::read_to_string(&args).unwrap();
        assert!(!argv.contains(SECRET), "{remote}: {argv}");
        assert!(argv.contains(remote.lines().next().unwrap()));
    }
}
