//! The Unix bootstrap on a Linux machine whose libc is checked before anything is
//! downloaded, and whose runtime must execute before it is installed. The real script runs
//! with fake `uname`, `getconf`, `ldd` and `curl` on PATH.
#![cfg(unix)]

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const RELOCATION_ERROR: &str = "Error relocating /home/vscode/.monocode-host/runtime/.install.nusPebem/unpacked/bin/node: fcntl64: symbol not found";

struct Machine {
    /// Output of `getconf GNU_LIBC_VERSION`; `None` makes getconf fail.
    getconf: Option<&'static str>,
    /// Output of `ldd --version`; `None` means there is no ldd.
    ldd: Option<&'static str>,
    /// Body of the packaged `monocode-host`.
    host: String,
}

struct Outcome {
    ok: bool,
    stderr: String,
    urls: Vec<String>,
    home: tempfile::TempDir,
}

fn working_host() -> String {
    format!(
        "#!/bin/sh\ncase \"$1\" in --version) echo '{}' ;; connection-info) echo '{{\"port\":3774}}' ;; esac\nexit 0\n",
        super::super::host_release::HOST_VERSION
    )
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(machine: &Machine) -> Outcome {
    use sha2::{Digest, Sha256};
    let work = tempfile::tempdir().unwrap();
    let pkg = work.path().join("pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    write_exec(&pkg.join("monocode-host"), &machine.host);
    let tarball = work.path().join("pkg.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&tarball)
        .arg("-C")
        .arg(&pkg)
        .arg("monocode-host")
        .status()
        .unwrap();
    assert!(status.success());
    let hash = format!("{:x}", Sha256::digest(std::fs::read(&tarball).unwrap()));
    std::fs::write(work.path().join("pkg.sha256"), format!("{hash}  pkg\n")).unwrap();
    let stubs = work.path().join("stubs");
    std::fs::create_dir_all(&stubs).unwrap();
    write_exec(
        &stubs.join("curl"),
        &format!(
            "#!/bin/sh\nurl=; out=\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=$2; shift ;; https://*) url=$1 ;; esac; shift; done\necho \"$url\" >> {log}\ncase \"$url\" in *.sha256) cp {sha} \"$out\" ;; *) cp {tar} \"$out\" ;; esac\n",
            log = work.path().join("urls.log").display(),
            sha = work.path().join("pkg.sha256").display(),
            tar = tarball.display(),
        ),
    );
    write_exec(
        &stubs.join("uname"),
        "#!/bin/sh\ncase \"$1\" in -m) echo x86_64 ;; *) echo Linux ;; esac\n",
    );
    write_exec(
        &stubs.join("getconf"),
        &match machine.getconf {
            Some(out) => format!("#!/bin/sh\necho '{out}'\n"),
            None => "#!/bin/sh\nexit 1\n".to_string(),
        },
    );
    write_exec(
        &stubs.join("ldd"),
        &match machine.ldd {
            Some(out) => format!("#!/bin/sh\ncat >&2 <<'EOF_LDD'\n{out}\nEOF_LDD\nexit 1\n"),
            None => "#!/bin/sh\nexit 127\n".to_string(),
        },
    );
    let mut script = bootstrap_script(HostPlatform::Unix);
    for (_, expected) in super::super::host_release::HOST_SHA256 {
        script = script.replace(expected, &hash);
    }
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new("sh")
        .arg("-s")
        .env("HOME", home.path())
        .env(
            "PATH",
            format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap()),
        )
        .env_remove("MONOCODE_HOST_FORCE_UPGRADE")
        .env_remove("MONOCODE_HOST_UPGRADE_IF_OLDER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let urls = std::fs::read_to_string(work.path().join("urls.log"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect();
    Outcome {
        ok: output.status.success(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        urls,
        home,
    }
}

/// Nothing of an install is left: no launcher, no runtime pointer, no runtime directory
/// (a half-installed `.install.*` or a moved package).
fn assert_nothing_installed(home: &Path) {
    let base = home.join(".monocode-host");
    assert!(!base.join("bin/monocode-host").exists());
    assert!(!base.join("runtime-path").exists());
    let leftovers: Vec<_> = std::fs::read_dir(base.join("runtime"))
        .map(|dir| dir.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
}

const MUSL_LDD: &str = "musl libc (x86_64)\nVersion 1.2.4\nDynamic Program Loader";
const GLIBC_LDD: &str = "ldd (Ubuntu GLIBC 2.35-0ubuntu3.8) 2.35";

#[test]
fn a_musl_machine_is_refused_before_anything_is_downloaded() {
    let outcome = run(&Machine {
        getconf: None,
        ldd: Some(MUSL_LDD),
        host: working_host(),
    });
    assert!(!outcome.ok);
    assert!(outcome.stderr.contains("musl libc"), "{}", outcome.stderr);
    assert!(outcome.stderr.contains("glibc 2.28"), "{}", outcome.stderr);
    assert!(outcome.urls.is_empty(), "downloaded: {:?}", outcome.urls);
    assert_nothing_installed(outcome.home.path());
}

#[test]
fn getconf_wins_over_a_musl_looking_ldd() {
    let outcome = run(&Machine {
        getconf: Some("glibc 2.35"),
        ldd: Some(MUSL_LDD),
        host: working_host(),
    });
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(outcome.urls.len(), 2, "{:?}", outcome.urls);
    assert!(outcome
        .home
        .path()
        .join(".monocode-host/bin/monocode-host")
        .is_file());
}

#[test]
fn a_glibc_ldd_and_an_unknown_libc_both_proceed() {
    for ldd in [Some(GLIBC_LDD), None] {
        let outcome = run(&Machine {
            getconf: None,
            ldd,
            host: working_host(),
        });
        assert!(outcome.ok, "{ldd:?}: {}", outcome.stderr);
    }
}

#[test]
fn a_glibc_older_than_2_28_is_refused_with_a_clear_message() {
    let outcome = run(&Machine {
        getconf: Some("glibc 2.17"),
        ldd: None,
        host: working_host(),
    });
    assert!(!outcome.ok);
    assert!(outcome.stderr.contains("glibc 2.17"), "{}", outcome.stderr);
    assert!(outcome.stderr.contains("2.28"), "{}", outcome.stderr);
    assert!(outcome.urls.is_empty());
    assert_nothing_installed(outcome.home.path());
    // 2.28 itself, and a longer minor, are accepted.
    for version in ["glibc 2.28", "glibc 2.39"] {
        let outcome = run(&Machine {
            getconf: Some(version),
            ldd: None,
            host: working_host(),
        });
        assert!(outcome.ok, "{version}: {}", outcome.stderr);
    }
}

#[test]
fn a_runtime_that_cannot_execute_is_reported_as_such_and_leaves_nothing() {
    let host = format!("#!/bin/sh\necho '{RELOCATION_ERROR}' >&2\nexit 127\n");
    let outcome = run(&Machine {
        getconf: None,
        ldd: None,
        host,
    });
    assert!(!outcome.ok);
    assert!(
        outcome
            .stderr
            .contains("The Monoptah Host runtime cannot run on this machine:"),
        "{}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("fcntl64: symbol not found"),
        "{}",
        outcome.stderr
    );
    assert!(
        !outcome.stderr.contains("version mismatch"),
        "{}",
        outcome.stderr
    );
    assert_nothing_installed(outcome.home.path());
}

#[test]
fn a_runtime_that_runs_but_prints_another_version_is_a_version_mismatch() {
    let outcome = run(&Machine {
        getconf: None,
        ldd: None,
        host: "#!/bin/sh\necho '0.0.1'\n".to_string(),
    });
    assert!(!outcome.ok);
    assert!(
        outcome.stderr.contains("Monoptah Host version mismatch."),
        "{}",
        outcome.stderr
    );
    assert!(!outcome.stderr.contains("cannot run"), "{}", outcome.stderr);
    assert_nothing_installed(outcome.home.path());
}
