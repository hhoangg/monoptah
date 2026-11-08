use super::docker::*;
#[cfg(unix)]
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

// ------------------------------------------------------- run_script / copy_from

use crate::cloud_sessions::backend::CloudBackend;
use crate::remote::SandboxTarget;
use crate::sandboxes::{SbxOutput, SbxRunner};
use std::sync::{Arc, Mutex};

struct NoSbx;
impl SbxRunner for NoSbx {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<SbxOutput, String> {
        panic!("the script and copy paths never call sbx: {args:?}")
    }
}

/// Runs the script with `sh -s` on this machine (the "container" is the local disk), or
/// replies with a canned answer.
struct Container {
    calls: Mutex<Vec<(String, String, String)>>,
    canned: Mutex<Option<ShellOutput>>,
}
impl Container {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(vec![]),
            canned: Mutex::new(None),
        })
    }
    fn say(&self, stdout: &str, stderr: &str, success: bool) {
        *self.canned.lock().unwrap() = Some(ShellOutput {
            success,
            stdout: stdout.into(),
            stderr: stderr.into(),
        });
    }
}
impl SandboxShell for Container {
    fn run(
        &self,
        id: &str,
        remote: &str,
        stdin: &str,
        on_line: &mut dyn FnMut(&str),
        abort: &dyn Fn() -> bool,
        _timeout: Duration,
    ) -> Result<ShellOutput, String> {
        self.calls
            .lock()
            .unwrap()
            .push((id.into(), remote.into(), stdin.into()));
        let out = match self.canned.lock().unwrap().clone() {
            Some(out) => out,
            None => {
                use std::io::Write;
                use std::process::{Command, Stdio};
                let mut child = Command::new("sh")
                    .arg("-s")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(stdin.as_bytes())
                    .unwrap();
                let result = child.wait_with_output().unwrap();
                ShellOutput {
                    success: result.status.success(),
                    stdout: String::from_utf8_lossy(&result.stdout).into(),
                    stderr: String::from_utf8_lossy(&result.stderr).into(),
                }
            }
        };
        for line in out.stdout.lines() {
            if abort() {
                return Err(SHELL_ABORTED.into());
            }
            on_line(line);
        }
        Ok(out)
    }
}

fn target() -> SandboxTarget {
    SandboxTarget {
        id: "sbx_001abc".into(),
        name: "box-1".into(),
        workspace: "/home/agent/workspaces/ws".into(),
        relay_port: 47200,
        config: None,
    }
}
fn backend(shell: &Arc<Container>) -> DockerBackend {
    let backend = DockerBackend::new(Arc::new(NoSbx)).with_shell(shell.clone());
    backend.remember_target(&target());
    backend
}
fn b64_lines(bytes: &[u8]) -> String {
    use base64::Engine as _;
    let text = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(76) {
        out.push_str(&format!(
            "MONOPTAH_B:{}\n",
            std::str::from_utf8(chunk).unwrap()
        ));
    }
    out
}

#[test]
fn run_script_execs_in_the_container_with_the_script_on_stdin() {
    let shell = Container::new();
    shell.say("key=value\n", "", true);
    let out = backend(&shell)
        .run_script("box-1", "echo secret-free\n", false)
        .unwrap();
    assert_eq!(out, "key=value\n");
    let calls = shell.calls.lock().unwrap().clone();
    assert_eq!(calls[0].0, "sbx_001abc");
    assert_eq!(
        calls[0].1,
        "$HOME/.devcontainers/bin/devcontainer exec --workspace-folder '/home/agent/workspaces/ws' -- sh -l -s"
    );
    assert_eq!(calls[0].2, "echo secret-free\n");
    // A failing script reports its own last lines, redacted.
    shell.say("", "boom ghp_SECRETSECRET123456 tail\n", false);
    let error = backend(&shell).run_script("box-1", "x", true).unwrap_err();
    assert!(
        error.contains("boom") && !error.contains("ghp_SECRETSECRET123456"),
        "{error}"
    );
}

#[test]
fn script_and_copy_need_a_connected_sandbox_and_the_ssh_seam() {
    let shell = Container::new();
    let backend_without_target = DockerBackend::new(Arc::new(NoSbx)).with_shell(shell.clone());
    let error = backend_without_target
        .run_script("box-1", "x", false)
        .unwrap_err();
    assert!(error.contains("not connected"), "{error}");
    let dir = tempfile::tempdir().unwrap();
    assert!(backend_without_target
        .copy_from("box-1", "/tmp/x", dir.path())
        .is_err());
    let no_shell = DockerBackend::new(Arc::new(NoSbx));
    no_shell.remember_target(&target());
    assert!(no_shell
        .run_script("box-1", "x", false)
        .unwrap_err()
        .contains("not available"));
    assert!(shell.calls.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn copy_from_streams_a_tar_and_unpacks_files_and_folders_but_not_links() {
    let shell = Container::new();
    let container = tempfile::tempdir().unwrap();
    let folder = container.path().join("evidence");
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    std::fs::write(folder.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(folder.join("sub/b.bin"), [0u8, 255, 7, 13, 10]).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", folder.join("link")).unwrap();
    let dest = tempfile::tempdir().unwrap();
    let remote = folder.to_str().unwrap();
    backend(&shell)
        .copy_from("box-1", remote, dest.path())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dest.path().join("evidence/a.txt")).unwrap(),
        "alpha\n"
    );
    assert_eq!(
        std::fs::read(dest.path().join("evidence/sub/b.bin")).unwrap(),
        [0u8, 255, 7, 13, 10]
    );
    assert!(
        std::fs::symlink_metadata(dest.path().join("evidence/link")).is_err(),
        "links are skipped, never followed"
    );
    // A single file arrives under its own name.
    backend(&shell)
        .copy_from("box-1", folder.join("a.txt").to_str().unwrap(), dest.path())
        .unwrap();
    assert!(dest.path().join("a.txt").is_file());
    // The path reached the script quoted, through the container exec.
    let calls = shell.calls.lock().unwrap().clone();
    assert!(calls[0].1.contains("devcontainer exec"));
    assert!(calls[0].2.contains(&format!("p='{remote}'")));
}

#[cfg(unix)]
#[test]
fn copy_from_reports_a_missing_path_in_the_form_the_callers_recognise() {
    let shell = Container::new();
    let dest = tempfile::tempdir().unwrap();
    let remote = "/definitely/not/here/evidence";
    let error = backend(&shell)
        .copy_from("box-1", remote, dest.path())
        .unwrap_err();
    assert!(
        crate::codespaces::is_missing_path(&error, remote),
        "{error}"
    );
}

#[test]
fn copy_from_refuses_unsafe_remote_paths_and_local_destinations() {
    let shell = Container::new();
    let dest = tempfile::tempdir().unwrap();
    let backend = backend(&shell);
    for bad in ["", "-rf", "/a/../b", "/a b", "/a;b", "/a/./b", "/a$(x)"] {
        assert!(
            backend.copy_from("box-1", bad, dest.path()).is_err(),
            "{bad}"
        );
    }
    assert!(backend
        .copy_from("box-1", "/tmp/x", std::path::Path::new("relative"))
        .is_err());
    assert!(backend
        .copy_from("box-1", "/tmp/x", &dest.path().join("missing"))
        .is_err());
    assert!(shell.calls.lock().unwrap().is_empty());
}

#[test]
fn copy_from_rejects_traversal_cut_off_streams_and_oversized_copies() {
    let shell = Container::new();
    let dest = tempfile::tempdir().unwrap();
    let inner = dest.path().join("inner");
    std::fs::create_dir(&inner).unwrap();
    // An archive entry that climbs out of the destination.
    let mut raw = vec![];
    {
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        let name = b"../evil.txt";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        let mut builder = tar::Builder::new(&mut raw);
        builder.append(&header, &b"hi"[..]).unwrap();
        builder.finish().unwrap();
    }
    shell.say(
        &format!("{}MONOPTAH_TAR_END={}\n", b64_lines(&raw), raw.len()),
        "",
        true,
    );
    let error = backend(&shell)
        .copy_from("box-1", "/tmp/x", &inner)
        .unwrap_err();
    assert!(error.contains("unsafe"), "{error}");
    assert!(!dest.path().join("evil.txt").exists());
    // No end marker: the connection was cut.
    shell.say(&b64_lines(&raw), "", true);
    let error = backend(&shell)
        .copy_from("box-1", "/tmp/x", &inner)
        .unwrap_err();
    assert!(error.contains("cut off"), "{error}");
    // Over the size limit.
    shell.say(
        &format!("{}MONOPTAH_TAR_END=4096\n", b64_lines(&vec![1u8; 4096])),
        "",
        true,
    );
    let small = DockerBackend::new(Arc::new(NoSbx))
        .with_shell(shell.clone())
        .with_copy_limit(1024);
    small.remember_target(&target());
    let error = small.copy_from("box-1", "/tmp/x", &inner).unwrap_err();
    assert!(error.contains("limit"), "{error}");
}

#[test]
fn prepare_evidence_runs_the_codespace_script_in_the_container() {
    let shell = Container::new();
    shell.say("", "", true);
    let backend = backend(&shell);
    assert!(backend.prepare_evidence("box-1", "/home/x/ws").is_err());
    assert!(shell.calls.lock().unwrap().is_empty());
    backend
        .prepare_evidence("box-1", "/workspaces/widgets")
        .unwrap();
    let calls = shell.calls.lock().unwrap().clone();
    assert!(calls[0].1.contains("devcontainer exec"));
    assert!(calls[0].2.contains("d='/workspaces/widgets'"));
    assert!(calls[0].2.contains(".evidence"));
    // A failing guide is reported (callers treat it as best effort).
    shell.say("", "CLAUDE.local.md left alone: it is a symlink\n", false);
    assert!(backend
        .prepare_evidence("box-1", "/workspaces/widgets")
        .is_err());
}

#[test]
fn copy_from_detects_bad_decodes_count_mismatches_stray_lines_and_missing_tools() {
    let shell = Container::new();
    let dest = tempfile::tempdir().unwrap();
    let mut raw = vec![];
    {
        let mut builder = tar::Builder::new(&mut raw);
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "ok.txt", &b"hi"[..])
            .unwrap();
        builder.finish().unwrap();
    }
    let good = b64_lines(&raw);
    // Stray stderr-like lines (even ones that look like data without the prefix) are ignored.
    shell.say(
        &format!(
            "B AAAA\nwarning: foo\n{good}MONOPTAH_TAR_END={}\n",
            raw.len()
        ),
        "",
        true,
    );
    backend(&shell)
        .copy_from("box-1", "/tmp/x", dest.path())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dest.path().join("ok.txt")).unwrap(),
        "hi"
    );
    // The staging file never stays behind.
    let left: Vec<_> = std::fs::read_dir(dest.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".monoptah-copy")
        })
        .collect();
    assert!(left.is_empty());
    // A data line that is not base64 is an error, not a silent gap.
    shell.say("MONOPTAH_B:!!notbase64!!\nMONOPTAH_TAR_END=3\n", "", true);
    let error = backend(&shell)
        .copy_from("box-1", "/tmp/x", dest.path())
        .unwrap_err();
    assert!(error.contains("damaged"), "{error}");
    // The announced byte count must match what arrived.
    shell.say(
        &format!("{good}MONOPTAH_TAR_END={}\n", raw.len() + 1),
        "",
        true,
    );
    let error = backend(&shell)
        .copy_from("box-1", "/tmp/x", dest.path())
        .unwrap_err();
    assert!(error.contains("incomplete"), "{error}");
    // A missing tool is named instead of producing an empty success.
    shell.say("", "MONOPTAH_MISSING_TOOL=base64\n", false);
    let error = backend(&shell)
        .copy_from("box-1", "/tmp/x", dest.path())
        .unwrap_err();
    assert!(error.contains("no `base64`"), "{error}");
}

#[cfg(unix)]
#[test]
fn skipped_links_are_reported_once() {
    let shell = Container::new();
    let container = tempfile::tempdir().unwrap();
    let folder = container.path().join("evidence");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "x").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", folder.join("link")).unwrap();
    let dest = tempfile::tempdir().unwrap();
    let backend = backend(&shell);
    backend
        .copy_from("box-1", folder.to_str().unwrap(), dest.path())
        .unwrap();
    let notes = backend.take_copy_notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("evidence/link"));
    assert!(backend.take_copy_notes().is_empty());
}
