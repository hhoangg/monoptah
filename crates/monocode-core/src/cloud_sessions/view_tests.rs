use super::view::*;
use super::view_cache::*;
use super::*;
use crate::codespaces::GhOutput;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

const ROOT: &str = "/workspaces/widgets";

struct NoGh;
impl GhRunner for NoGh {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<GhOutput, String> {
        panic!("viewing a file must not call gh: {args:?}");
    }
}

/// A Host serving `files.readBytes` / `files.read` from memory.
#[derive(Default)]
struct FileHost {
    files: Mutex<HashMap<String, Vec<u8>>>,
    log: Mutex<Vec<String>>,
    /// The `cwd` param of every file read (`None`: the request carried none).
    cwds: Mutex<Vec<Option<String>>>,
    disconnected: AtomicBool,
    legacy: bool,
    /// Every call answers like a machine the connection store no longer knows.
    gone: bool,
    describes: std::sync::atomic::AtomicUsize,
    /// Report this size for every file's first read (a huge file without storing it).
    fake_size: Option<u64>,
    /// Drop the last byte of every file, like a file truncated during the read.
    truncate: bool,
}
impl FileHost {
    fn put(&self, rel: &str, bytes: &[u8]) {
        self.files
            .lock()
            .unwrap()
            .insert(rel.into(), bytes.to_vec());
    }
    fn cwds(&self) -> Vec<Option<String>> {
        self.cwds.lock().unwrap().clone()
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}
impl Host for FileHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(vec![Machine {
            id: "m-octo-abc".into(),
            name: "octo-abc".into(),
            endpoint: "codespace://octo-abc".into(),
            environment_id: "env-octo-abc".into(),
            ssh: None,
            codespace: Some("octo-abc".into()),
            sandbox: None,
        }])
    }
    fn connect(&self, _: &str, _: &str, _: &mut dyn FnMut(&str)) -> Result<Machine, String> {
        panic!("viewing a file must not connect");
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        panic!("viewing a file must not wake the workspace");
    }
    fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String> {
        assert_eq!(machine_id, "m-octo-abc");
        if self.gone {
            return Err("Machine is no longer connected".into());
        }
        if method == "environment.describe" {
            self.describes.fetch_add(1, Ordering::SeqCst);
            if self.disconnected.load(Ordering::SeqCst) {
                return Err(remote::CODESPACE_NOT_CONNECTED.into());
            }
            let mut capabilities = vec!["files.read", "files.list"];
            if !self.legacy {
                capabilities.push("files.readBytes");
            }
            return Ok(json!({ "capabilities": capabilities }));
        }
        assert_eq!(params["projectId"], "project-1");
        let path = params["path"].as_str().unwrap().to_string();
        self.log.lock().unwrap().push(format!("{method} {path}"));
        self.cwds
            .lock()
            .unwrap()
            .push(params["cwd"].as_str().map(str::to_owned));
        if self.disconnected.load(Ordering::SeqCst) {
            return Err(remote::CODESPACE_NOT_CONNECTED.into());
        }
        let files = self.files.lock().unwrap();
        let Some(bytes) = files.get(&path) else {
            return Err(format!("ENOENT: no such file {path}"));
        };
        match method {
            "files.readBytes" if self.legacy => panic!("a Host without the capability was asked"),
            "files.readBytes" => {
                let offset = params["offset"].as_u64().unwrap() as usize;
                let length = params["length"].as_u64().unwrap().min(CHUNK_BYTES) as usize;
                let size = self.fake_size.unwrap_or(bytes.len() as u64);
                let end = if self.fake_size.is_some() {
                    offset + length
                } else {
                    (offset + length).min(bytes.len())
                };
                let mut part: Vec<u8> = if self.fake_size.is_some() {
                    vec![0; length]
                } else {
                    bytes[offset..end].to_vec()
                };
                if self.truncate && end == bytes.len() && !part.is_empty() {
                    part.pop();
                }
                Ok(json!({
                    "data": base64::engine::general_purpose::STANDARD.encode(&part),
                    "offset": offset + part.len(),
                    "size": size,
                }))
            }
            "files.read" => {
                if bytes.len() > 1024 * 1024 || bytes.contains(&0) {
                    return Err("File is too large to preview".into());
                }
                Ok(Value::String(String::from_utf8_lossy(bytes).into_owned()))
            }
            other => panic!("unexpected {other}"),
        }
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        panic!("viewing a file must not disconnect");
    }
}
use base64::Engine;

fn setup(host: FileHost, dir: &Path) -> (CloudSessions, Arc<FileHost>, i64) {
    let host = Arc::new(host);
    let store = Arc::new(Mutex::new(Store::open(&dir.join("sessions.db")).unwrap()));
    let svc = CloudSessions::new(Arc::new(NoGh), host.clone(), store.clone());
    let project = store
        .lock()
        .unwrap()
        .remember_cloud_project(
            "env-octo-abc",
            &json!({"id":"project-1","cwd":ROOT}),
            "/Users/me/widgets",
            "octo-abc",
        )
        .unwrap();
    let session = store
        .lock()
        .unwrap()
        .create("claude", &project.key)
        .unwrap();
    (svc, host, session.id)
}

fn open(svc: &CloudSessions, id: i64, raw: &str) -> Result<Opened, ViewError> {
    let file = svc.resolve_cloud_file(id, raw)?;
    svc.open_cloud_file(&file, &AtomicBool::new(false))
}

#[test]
fn paths_resolve_inside_the_workspace_and_are_refused_outside() {
    let ok = |raw: &str| resolve_workspace_path(raw, ROOT).unwrap();
    assert_eq!(
        ok("/workspaces/widgets/.evidence/report.html"),
        ".evidence/report.html"
    );
    assert_eq!(ok(".evidence/shot.png"), ".evidence/shot.png");
    assert_eq!(ok("./a//b/../c.md"), "a/c.md");
    assert_eq!(ok("/workspaces/widgets/a/../b.txt"), "b.txt");
    // A sandbox without a devcontainer keeps its checkout elsewhere.
    assert_eq!(
        resolve_workspace_path(
            "/home/agent/workspaces/repo/.evidence/shot.png",
            "/home/agent/workspaces/repo"
        )
        .unwrap(),
        ".evidence/shot.png"
    );
    for raw in [
        "/etc/passwd",
        "/workspaces/other/x.md",
        "/workspaces/widgets-evil/x.md",
        "../x.md",
        "a/../../x.md",
        "/workspaces/widgets/../../etc/passwd",
        "/workspaces/widgets",
        "",
    ] {
        assert!(
            matches!(
                resolve_workspace_path(raw, ROOT),
                Err(ViewError::Refused(_))
            ),
            "{raw}"
        );
    }
    let outside = resolve_workspace_path("/etc/passwd", ROOT)
        .unwrap_err()
        .to_string();
    assert!(outside.contains("outside the cloud workspace"), "{outside}");
    for raw in [".git/config", "a/.GIT/x", "a\\b.md", "a\0b"] {
        assert!(
            matches!(
                resolve_workspace_path(raw, ROOT),
                Err(ViewError::Refused(_))
            ),
            "{raw:?}"
        );
    }
}

#[test]
fn a_session_that_is_not_connected_asks_to_reconnect_and_wakes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put(".evidence/report.html", b"<p>hi</p>");
    host.disconnected.store(true, Ordering::SeqCst);
    let (svc, host, id) = setup(host, dir.path());
    let error = open(&svc, id, "/workspaces/widgets/.evidence/report.html").unwrap_err();
    assert_eq!(
        error,
        ViewError::NotConnected("Reconnect to view report.html".into())
    );
    // One read-only capability check; the fake panics on ensure/connect/disconnect.
    assert!(host.log().is_empty());
    assert_eq!(host.describes.load(Ordering::SeqCst), 1);
    let file = svc.resolve_cloud_file(id, "x.bin").unwrap();
    let error = svc
        .download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap_err();
    assert!(matches!(error, ViewError::NotConnected(_)));
}

#[test]
fn a_non_cloud_session_or_outside_path_sends_no_request() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, host, id) = setup(FileHost::default(), dir.path());
    let error = svc.resolve_cloud_file(id, "/etc/passwd").unwrap_err();
    assert!(matches!(error, ViewError::Refused(_)));
    let local = svc
        .store()
        .unwrap()
        .create("claude", "/Users/me/local")
        .unwrap();
    assert!(svc.resolve_cloud_file(local.id, "a.md").is_err());
    assert!(host.log().is_empty());
}

#[test]
fn small_previewable_files_open_in_memory_by_type() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    let png = vec![7u8; 700_000];
    host.put("shot.png", &png);
    host.put("notes.md", b"# Title\n");
    host.put("src/main.rs", b"fn main() {}\n");
    host.put("page.html", b"<h1>Plain</h1>");
    host.put("blob.txt", b"a\0b");
    host.put("archive.zip", b"PK");
    let (svc, host, id) = setup(host, dir.path());
    assert_eq!(
        open(&svc, id, "shot.png").unwrap(),
        Opened::Image {
            bytes: png,
            mime: "image/png"
        }
    );
    // 700 KB needs two 384 KB reads.
    assert_eq!(host.log().len(), 2);
    assert_eq!(
        open(&svc, id, "/workspaces/widgets/notes.md").unwrap(),
        Opened::Text {
            kind: FileKind::Markdown,
            text: "# Title\n".into()
        }
    );
    assert_eq!(
        open(&svc, id, "src/main.rs").unwrap(),
        Opened::Text {
            kind: FileKind::Text,
            text: "fn main() {}\n".into()
        }
    );
    assert_eq!(
        open(&svc, id, "page.html").unwrap(),
        Opened::Html {
            html: "<h1>Plain</h1>".into()
        }
    );
    // A "text" file with NUL bytes and a non-previewable type go to the download path.
    for (name, size) in [("blob.txt", 3), ("archive.zip", 2)] {
        match open(&svc, id, name).unwrap() {
            Opened::Download(plan) => {
                assert_eq!(plan.size, size);
                assert_eq!(plan.reason, PlanReason::NotPreviewable);
                assert!(!plan.needs_confirm);
            }
            other => panic!("{other:?}"),
        }
    }
    // The preview never wrote anything to the session folder.
    assert!(!dir
        .path()
        .join("sessions.db.threads")
        .join("cloud-cache")
        .exists());
}

#[test]
fn an_older_host_still_opens_small_text_but_not_binary_files() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost {
        legacy: true,
        ..Default::default()
    };
    host.put("notes.md", b"# Old host\n");
    host.put("shot.png", &[1u8; 100]);
    let (svc, host, id) = setup(host, dir.path());
    assert_eq!(
        open(&svc, id, "notes.md").unwrap(),
        Opened::Text {
            kind: FileKind::Markdown,
            text: "# Old host\n".into()
        }
    );
    assert_eq!(host.log(), vec!["files.read notes.md"]);
    // Capabilities, not error text, decide: images need the copy fallback.
    assert_eq!(
        open(&svc, id, "shot.png").unwrap(),
        Opened::CopyOnly {
            kind: FileKind::Image
        }
    );
    let text = ViewError::OldHost.to_string();
    assert!(
        text.contains("updates itself the next time this workspace connects")
            && !text.contains("Connections settings"),
        "{text}"
    );
    assert!(text.contains("download a copy"), "{text}");
}

#[test]
fn a_machine_the_connection_store_forgot_counts_as_not_connected() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _host, id) = setup(
        FileHost {
            gone: true,
            ..Default::default()
        },
        dir.path(),
    );
    assert_eq!(
        open(&svc, id, "a.png").unwrap_err(),
        ViewError::NotConnected("Reconnect to view a.png".into())
    );
}

#[test]
fn remote_links_must_name_this_sessions_machine() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _host, id) = setup(FileHost::default(), dir.path());
    let file = svc
        .resolve_cloud_file(id, "remote://env-octo-abc/workspaces/widgets/r.md")
        .unwrap();
    assert_eq!(file.rel, "r.md");
    let error = svc
        .resolve_cloud_file(id, "remote://env-other/workspaces/widgets/r.md")
        .unwrap_err();
    assert!(matches!(error, ViewError::Refused(_)), "{error:?}");
}

#[test]
fn only_files_that_cannot_run_are_opened_by_the_default_app() {
    for name in [
        "x.app",
        "run.COMMAND",
        "a.sh",
        "i.pkg",
        "d.dmg",
        "w.webloc",
        "t.terminal",
    ] {
        assert!(runs_code(name), "{name}");
    }
    for name in ["a.png", "r.html", "notes.md", "data.bin", "script"] {
        assert!(!runs_code(name), "{name}");
    }
}

/// A backend that only knows its state and `copy_from`.
struct FakeBackend {
    raw: &'static str,
    copy: CopyFn,
    calls: Mutex<Vec<String>>,
}
type CopyFn = Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;
impl backend::CloudBackend for FakeBackend {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::Codespaces
    }
    fn display_state(&self, raw: &str) -> CloudState {
        CloudState::from_gh(raw)
    }
    fn is_stopped(&self, raw: &str) -> bool {
        raw == "Shutdown" || raw == "ShuttingDown"
    }
    fn is_deleted_state(&self, _: &str) -> bool {
        false
    }
    fn is_missing_error(&self, _: &str) -> bool {
        false
    }
    fn state(&self, _: &str) -> Result<String, String> {
        self.calls.lock().unwrap().push("state".into());
        Ok(self.raw.into())
    }
    fn exists(&self, _: &str) -> Result<bool, String> {
        Ok(true)
    }
    fn stop(&self, _: &str) -> Result<(), String> {
        panic!("not used")
    }
    fn delete(&self, _: &str) -> Result<(), String> {
        panic!("not used")
    }
    fn rebuild(&self, _: &str) -> Result<(), String> {
        panic!("not used")
    }
    fn web_url(&self, _: &str) -> Option<Result<String, String>> {
        None
    }
    fn run_script(&self, _: &str, _: &str, _: bool) -> Result<String, String> {
        panic!("not used")
    }
    fn copy_from(&self, _: &str, remote: &str, local: &Path) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("copy {remote}"));
        (self.copy)(local)
    }
    fn prepare_evidence(&self, _: &str, _: &str) -> Result<(), String> {
        panic!("not used")
    }
}

fn with_backend(
    svc: CloudSessions,
    raw: &'static str,
    copy: CopyFn,
) -> (CloudSessions, Arc<FakeBackend>) {
    let backend = Arc::new(FakeBackend {
        raw,
        copy,
        calls: Mutex::new(vec![]),
    });
    (svc.with_backend(backend.clone()), backend)
}

fn old_host_setup(dir: &Path) -> (CloudSessions, Arc<FileHost>, i64) {
    let host = FileHost {
        legacy: true,
        ..Default::default()
    };
    host.put("out/shot.png", &[1u8; 10]);
    setup(host, dir)
}

fn copy_file(local: &Path) -> Result<(), String> {
    std::fs::write(local.join("shot.png"), b"PNGDATA").map_err(|e| e.to_string())
}

#[test]
fn the_copy_fallback_runs_only_on_request_into_the_cache_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _host, id) = old_host_setup(dir.path());
    let (svc, backend) = with_backend(svc, "Available", Box::new(copy_file));
    let file = svc.resolve_cloud_file(id, "out/shot.png").unwrap();
    // Opening never copies; it only reports that the copy is the way.
    assert_eq!(
        svc.open_cloud_file(&file, &AtomicBool::new(false)).unwrap(),
        Opened::CopyOnly {
            kind: FileKind::Image
        }
    );
    assert!(backend.calls.lock().unwrap().is_empty());
    let got = svc.copy_cloud_file(&file, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        *backend.calls.lock().unwrap(),
        vec![
            "state".to_string(),
            "copy /workspaces/widgets/out/shot.png".to_string()
        ]
    );
    let cache = file.cache_dir.clone().unwrap();
    assert_eq!(got.path, cache.join("out").join("shot.png"));
    assert_eq!(std::fs::read(&got.path).unwrap(), b"PNGDATA");
    assert_eq!(got.size, 7);
    // The temporary folder is gone.
    let names: Vec<String> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["out".to_string()]);
}

#[test]
fn the_copy_fallback_refuses_a_workspace_that_is_not_running_or_not_reachable() {
    let dir = tempfile::tempdir().unwrap();
    for raw in ["Shutdown", "ShuttingDown", "Starting"] {
        let (svc, _host, id) = old_host_setup(dir.path());
        let (svc, backend) = with_backend(svc, raw, Box::new(copy_file));
        let file = svc.resolve_cloud_file(id, "out/shot.png").unwrap();
        let error = svc
            .copy_cloud_file(&file, &AtomicBool::new(false))
            .unwrap_err();
        assert!(
            matches!(error, ViewError::NotConnected(_)),
            "{raw}: {error:?}"
        );
        assert_eq!(
            *backend.calls.lock().unwrap(),
            vec!["state".to_string()],
            "{raw}"
        );
    }
    // Tunnel down: not even the state is read.
    let host = FileHost {
        legacy: true,
        ..Default::default()
    };
    host.disconnected.store(true, Ordering::SeqCst);
    let (svc, _host, id) = setup(host, dir.path());
    let (svc, backend) = with_backend(svc, "Available", Box::new(copy_file));
    let file = svc.resolve_cloud_file(id, "out/shot.png").unwrap();
    assert!(matches!(
        svc.copy_cloud_file(&file, &AtomicBool::new(false)),
        Err(ViewError::NotConnected(_))
    ));
    assert!(backend.calls.lock().unwrap().is_empty());
}

#[test]
fn the_copy_fallback_refuses_folders_and_links() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _host, id) = old_host_setup(dir.path());
    let (svc, _backend) = with_backend(
        svc,
        "Available",
        Box::new(|local| std::fs::create_dir(local.join("shot.png")).map_err(|e| e.to_string())),
    );
    let file = svc.resolve_cloud_file(id, "out/shot.png").unwrap();
    let error = svc
        .copy_cloud_file(&file, &AtomicBool::new(false))
        .unwrap_err();
    assert!(matches!(error, ViewError::Refused(_)), "{error:?}");
    #[cfg(unix)]
    {
        let (svc, _host, id) = old_host_setup(dir.path());
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();
        let target = secret.clone();
        let (svc, _backend) = with_backend(
            svc,
            "Available",
            Box::new(move |local| {
                std::os::unix::fs::symlink(&target, local.join("shot.png"))
                    .map_err(|e| e.to_string())
            }),
        );
        let file = svc.resolve_cloud_file(id, "out/shot.png").unwrap();
        let error = svc
            .copy_cloud_file(&file, &AtomicBool::new(false))
            .unwrap_err();
        assert!(matches!(error, ViewError::Refused(_)), "{error:?}");
        assert!(!file.cache_dir.unwrap().join("out/shot.png").exists());
    }
}

#[test]
fn a_cancelled_copy_keeps_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _host, id) = old_host_setup(dir.path());
    let (svc, _backend) = with_backend(svc, "Available", Box::new(copy_file));
    let file = svc.resolve_cloud_file(id, "out/shot.png").unwrap();
    let error = svc
        .copy_cloud_file(&file, &AtomicBool::new(true))
        .unwrap_err();
    assert_eq!(error, ViewError::Cancelled);
    assert_eq!(
        std::fs::read_dir(file.cache_dir.unwrap()).map_or(0, |d| d.count()),
        0
    );
}

#[test]
fn downloaded_pages_get_a_content_security_policy_in_the_cached_copy_only() {
    assert!(
        with_csp("<html><head><title>t</title></head>").starts_with(&format!(
            "<html><head><meta http-equiv=\"Content-Security-Policy\" content=\"{PAGE_CSP}\">"
        ))
    );
    assert!(with_csp("<!DOCTYPE html><p>x").contains("<!DOCTYPE html><meta http-equiv"));
    assert!(with_csp("<p>x").starts_with("<meta http-equiv"));
    // `<header>` is not `<head>`.
    assert!(with_csp("<header>h</header>").starts_with("<meta"));
    for needed in [
        "connect-src 'none'",
        "form-action 'none'",
        "default-src 'none'",
    ] {
        assert!(PAGE_CSP.contains(needed));
    }
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put("p.html", b"<html><head></head><body>hi</body></html>");
    let (svc, _host, id) = setup(host, dir.path());
    let file = svc.resolve_cloud_file(id, "p.html").unwrap();
    let got = svc
        .download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap();
    let cached = std::fs::read_to_string(&got.path).unwrap();
    assert!(
        cached.contains(PAGE_CSP) && cached.contains("<body>hi</body>"),
        "{cached}"
    );
    assert_eq!(got.size, 41);
}

#[test]
fn temporary_names_never_collide_with_real_files_and_eviction_spares_unfinished_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put("x.cloud-part", b"real file");
    host.put("x", b"other");
    let (svc, _host, id) = setup(host, dir.path());
    let run = |raw: &str| {
        let file = svc.resolve_cloud_file(id, raw).unwrap();
        svc.download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
            .unwrap()
    };
    assert_eq!(
        std::fs::read(run("x.cloud-part").path).unwrap(),
        b"real file"
    );
    assert_eq!(std::fs::read(run("x").path).unwrap(), b"other");
    let cache = dir.path().join("sessions.db.threads");
    let _ = cache;
    // An unfinished download (hidden temp name) survives any eviction.
    let root = dir.path().join("9/cloud-cache");
    aged(&root.join(".monoptah-part-1-1"), 500, 9_000);
    aged(&root.join(".monoptah-copy-1-1/file"), 500, 9_000);
    aged(&root.join("done.bin"), 500, 8_000);
    assert_eq!(evict(std::slice::from_ref(&root), 0, &[]), 500);
    assert!(root.join(".monoptah-part-1-1").exists());
    assert!(root.join(".monoptah-copy-1-1/file").exists());
    assert!(!root.join("done.bin").exists());
}

#[test]
fn html_assets_are_inlined_through_the_host_without_touching_disk() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put(
        ".evidence/report.html",
        concat!(
            r#"<link rel="stylesheet" href="css/site.css">"#,
            r#"<style>.hero{background:url('img/bg.png')}</style>"#,
            r#"<img src="img/shot.png" alt="x"><img src="https://example.com/remote.png">"#,
            r#"<img src="../../../etc/passwd"><img src="missing.png">"#,
            r#"<div style="background:url(img/bg.png)"></div>"#,
            r#"<script src="js/app.js"></script><p>done</p>"#,
        )
        .as_bytes(),
    );
    host.put(
        ".evidence/css/site.css",
        b"body{background:url(../img/bg.png)} @font-face{src:url(\"f.woff2\")}",
    );
    host.put(".evidence/css/f.woff2", b"FONT");
    host.put(".evidence/img/bg.png", b"BG");
    host.put(".evidence/img/shot.png", b"SHOT");
    host.put(".evidence/js/app.js", b"var x='</script>';");
    let (svc, _host, id) = setup(host, dir.path());
    let Opened::Html { html } = open(&svc, id, ".evidence/report.html").unwrap() else {
        panic!("expected an inlined page");
    };
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    assert!(
        html.contains(&format!("src=\"data:image/png;base64,{}\"", b64("SHOT"))),
        "{html}"
    );
    assert!(
        html.contains(&format!("url(\"data:image/png;base64,{}\")", b64("BG"))),
        "{html}"
    );
    assert!(
        html.contains(&format!("url(\"data:font/woff2;base64,{}\")", b64("FONT"))),
        "{html}"
    );
    assert!(!html.contains("css/site.css"), "{html}");
    assert!(html.contains("<style>body{"), "{html}");
    assert!(
        html.contains("<script>var x='<\\/script>';</script><p>done</p>"),
        "{html}"
    );
    // External, outside and missing references stay as they were.
    assert!(html.contains("https://example.com/remote.png"));
    assert!(html.contains("../../../etc/passwd"));
    assert!(html.contains("src=\"missing.png\""));
    assert!(!html.contains("img/shot.png"));
}

#[test]
fn html_whose_assets_exceed_the_memory_cap_goes_to_the_download_path() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put("big.html", br#"<img src="huge.png">"#);
    host.put("huge.png", &vec![1u8; (MAX_INLINE_BYTES as usize) + 10]);
    let (svc, _host, id) = setup(host, dir.path());
    match open(&svc, id, "big.html").unwrap() {
        Opened::Download(plan) => {
            assert_eq!(plan.reason, PlanReason::HtmlAssets);
            assert_eq!(plan.kind, FileKind::Html);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_large_file_downloads_into_the_session_cache_with_verification() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    let big: Vec<u8> = (0..(MAX_MEMORY_BYTES as usize + 1_000_000))
        .map(|i| (i % 251) as u8)
        .collect();
    host.put("out/big.png", &big);
    let (svc, host, id) = setup(host, dir.path());
    let file = svc
        .resolve_cloud_file(id, "/workspaces/widgets/out/big.png")
        .unwrap();
    let plan = match svc.open_cloud_file(&file, &AtomicBool::new(false)).unwrap() {
        Opened::Download(plan) => plan,
        other => panic!("{other:?}"),
    };
    assert_eq!(plan.reason, PlanReason::TooLarge);
    assert_eq!(plan.size, big.len() as u64);
    assert_eq!(host.log().len(), 1, "only the size probe was read");
    let mut last = (0, 0);
    let got = svc
        .download_cloud_file(&file, false, &AtomicBool::new(false), &mut |done, total| {
            assert!(done >= last.0 && done <= total);
            last = (done, total);
        })
        .unwrap();
    assert_eq!(last, (big.len() as u64, big.len() as u64));
    let cache = file.cache_dir.clone().unwrap();
    assert_eq!(got.path, cache.join("out").join("big.png"));
    assert_eq!(got.cache_dir, cache);
    assert_eq!(std::fs::read(&got.path).unwrap(), big);
    let leftovers: Vec<String> = std::fs::read_dir(got.path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, vec!["big.png".to_string()]);
}

#[test]
fn cancelling_a_download_leaves_no_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put("big.bin", &vec![3u8; 2_000_000]);
    let (svc, _host, id) = setup(host, dir.path());
    let file = svc.resolve_cloud_file(id, "big.bin").unwrap();
    let cancel = AtomicBool::new(false);
    let error = svc
        .download_cloud_file(&file, false, &cancel, &mut |done, _| {
            if done > 400_000 {
                cancel.store(true, Ordering::SeqCst);
            }
        })
        .unwrap_err();
    assert_eq!(error, ViewError::Cancelled);
    let cache = file.cache_dir.unwrap();
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
}

#[test]
fn a_download_that_ends_short_is_rejected_and_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost {
        truncate: true,
        ..Default::default()
    };
    host.put("a.bin", &vec![3u8; 900_000]);
    let (svc, _host, id) = setup(host, dir.path());
    let file = svc.resolve_cloud_file(id, "a.bin").unwrap();
    let error = svc
        .download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap_err();
    assert!(matches!(error, ViewError::Failed(_)), "{error:?}");
    assert_eq!(
        std::fs::read_dir(file.cache_dir.unwrap()).unwrap().count(),
        0
    );
}

#[test]
fn a_file_over_the_threshold_needs_confirmation_before_any_byte_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let size = CONFIRM_BYTES + 1;
    let host = FileHost {
        fake_size: Some(size),
        ..Default::default()
    };
    host.put("huge.bin", b"x");
    let (svc, _host, id) = setup(host, dir.path());
    let file = svc.resolve_cloud_file(id, "huge.bin").unwrap();
    match svc.open_cloud_file(&file, &AtomicBool::new(false)).unwrap() {
        Opened::Download(plan) => assert!(plan.needs_confirm && plan.size == size),
        other => panic!("{other:?}"),
    }
    let error = svc
        .download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap_err();
    assert_eq!(error, ViewError::NeedsConfirm { size });
    assert!(!file.cache_dir.unwrap().exists());
}

#[test]
fn an_html_download_also_fetches_the_assets_it_references() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put(
        "site/index.html",
        br#"<link rel=stylesheet href="s.css"><img src="/workspaces/widgets/img/a.png"><img src="gone.png">"#,
    );
    host.put("site/s.css", b"a{background:url(bg.png)}");
    host.put("site/bg.png", b"BG");
    host.put("img/a.png", b"A");
    let (svc, _host, id) = setup(host, dir.path());
    let file = svc.resolve_cloud_file(id, "site/index.html").unwrap();
    let got = svc
        .download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap();
    let cache = got.cache_dir;
    assert_eq!(got.path, cache.join("site/index.html"));
    assert_eq!(
        std::fs::read(cache.join("site/s.css")).unwrap(),
        b"a{background:url(bg.png)}"
    );
    assert_eq!(std::fs::read(cache.join("site/bg.png")).unwrap(), b"BG");
    assert_eq!(std::fs::read(cache.join("img/a.png")).unwrap(), b"A");
    assert!(!cache.join("site/gone.png").exists());
}

#[test]
fn cache_paths_cannot_escape_the_cache_folder() {
    let cache = Path::new("/data/x.threads/1/cloud-cache");
    assert_eq!(
        cache_path(cache, "a/b.png").unwrap(),
        cache.join("a").join("b.png")
    );
    for rel in [
        "../x",
        "a/../b",
        "/abs",
        "a//b",
        "",
        ".",
        "a/./b",
        ".git/config",
        "a\\b",
        "a\0b",
        "c:evil",
    ] {
        assert!(cache_path(cache, rel).is_err(), "{rel:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_link_inside_the_cache_is_never_followed() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put("sub/x.bin", b"data");
    let (svc, _host, id) = setup(host, dir.path());
    let file = svc.resolve_cloud_file(id, "sub/x.bin").unwrap();
    let cache = file.cache_dir.clone().unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    std::os::unix::fs::symlink(&outside, cache.join("sub")).unwrap();
    let error = svc
        .download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap_err();
    assert!(matches!(error, ViewError::Failed(_)), "{error:?}");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

fn aged(path: &Path, size: usize, age_secs: u64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![0u8; size]).unwrap();
    let at = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

#[test]
fn eviction_removes_the_oldest_files_across_sessions_and_spares_the_kept_one() {
    let dir = tempfile::tempdir().unwrap();
    let one: PathBuf = dir.path().join("1/cloud-cache");
    let two: PathBuf = dir.path().join("2/cloud-cache");
    aged(&one.join("old/a.bin"), 400, 3000);
    aged(&two.join("b.bin"), 400, 2000);
    aged(&one.join("c.bin"), 400, 1000);
    let kept = two.join("new.bin");
    aged(&kept, 400, 4000);
    let dirs = vec![one.clone(), two.clone()];
    // 1600 bytes, cap 900: a (oldest unkept) and then b go; the kept file is older but stays.
    let removed = evict(&dirs, 900, std::slice::from_ref(&kept));
    assert_eq!(removed, 800);
    assert!(!one.join("old/a.bin").exists() && !two.join("b.bin").exists());
    assert!(one.join("c.bin").exists() && kept.exists());
    // The emptied folder is tidied but the cache roots stay.
    assert!(!one.join("old").exists());
    assert!(one.exists() && two.exists());
    // Under the cap nothing is touched.
    assert_eq!(evict(&dirs, 10_000, &[]), 0);
}

#[test]
fn downloads_trim_the_app_wide_cache_and_archive_or_delete_clear_it() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put("a.bin", b"12345");
    let (svc, _host, id) = setup(host, dir.path());
    let file = svc.resolve_cloud_file(id, "a.bin").unwrap();
    let cache = file.cache_dir.clone().unwrap();
    // An old file of another session's cache is far above the cap: it is evicted.
    let other = cache
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("999/cloud-cache/old.bin");
    aged(&other, 10, 100_000);
    let store_dirs = svc.store().unwrap().cloud_cache_dirs();
    assert!(store_dirs.contains(&other.parent().unwrap().to_path_buf()));
    assert_eq!(evict(&store_dirs, 5, &[]), 10);
    svc.download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap();
    assert!(cache.join("a.bin").exists());
    // Archive: the cloud cache goes, the rest of the session folder stays.
    let evidence = cache.parent().unwrap().join("evidence/shot.png");
    aged(&evidence, 3, 10);
    svc.clear_cloud_cache(id).unwrap();
    assert!(!cache.exists() && evidence.exists());
    svc.clear_cloud_cache(id).unwrap();
    // Forgetting the session (delete) removes the cache too; saved evidence is kept as before.
    svc.download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap();
    assert!(cache.join("a.bin").exists());
    svc.store().unwrap().delete_remote_session(id).unwrap();
    assert!(!cache.exists() && evidence.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn downloaded_files_are_quarantined_on_macos() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put("tool.bin", b"abc");
    let (svc, _host, id) = setup(host, dir.path());
    let file = svc.resolve_cloud_file(id, "tool.bin").unwrap();
    let got = svc
        .download_cloud_file(&file, false, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap();
    let out = std::process::Command::new("xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(&got.path)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("0083;"));
}

const WORKTREE: &str = "/workspaces/widgets-feature";

/// Records `worktree` (as the store keeps it: `remote://<env>/<path>`) for session `id`.
fn set_worktree(svc: &CloudSessions, id: i64, worktree: Option<&str>, removed: bool) {
    let settings = crate::agent::Settings {
        worktree_cwd: worktree.map(|path| format!("remote://env-octo-abc{path}")),
        worktree_removed: removed,
        ..Default::default()
    };
    svc.store()
        .unwrap()
        .save_agent_settings(id, &settings)
        .unwrap();
}

#[test]
fn reads_carry_the_worktree_cwd_and_stay_inside_it() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost::default();
    host.put("src/main.rs", b"fn main() {}\n");
    let (svc, host, id) = setup(host, dir.path());
    set_worktree(&svc, id, Some(WORKTREE), false);
    let file = svc
        .resolve_cloud_file(id, &format!("{WORKTREE}/src/main.rs"))
        .unwrap();
    assert_eq!(file.cwd.as_deref(), Some(WORKTREE));
    assert_eq!(file.rel, "src/main.rs");
    svc.open_cloud_file(&file, &AtomicBool::new(false)).unwrap();
    assert_eq!(host.log(), vec!["files.readBytes src/main.rs"]);
    assert_eq!(host.cwds(), vec![Some(WORKTREE.to_owned())]);
    // The project root is outside this session's worktree: refused, nothing sent.
    let outside = svc
        .resolve_cloud_file(id, &format!("{ROOT}/src/main.rs"))
        .unwrap_err();
    assert!(matches!(outside, ViewError::Refused(_)), "{outside:?}");
    assert_eq!(host.log().len(), 1);
}

#[test]
fn the_older_text_read_also_carries_the_worktree_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let host = FileHost {
        legacy: true,
        ..Default::default()
    };
    host.put("notes.md", b"# Notes\n");
    let (svc, host, id) = setup(host, dir.path());
    set_worktree(&svc, id, Some(WORKTREE), false);
    open(&svc, id, "notes.md").unwrap();
    assert_eq!(host.log(), vec!["files.read notes.md"]);
    assert_eq!(host.cwds(), vec![Some(WORKTREE.to_owned())]);
}

#[test]
fn no_cwd_is_sent_without_a_usable_worktree() {
    let cases = [
        (None, false),
        // A removed worktree falls back to the project folder.
        (Some(WORKTREE), true),
        // A recorded folder outside the workspace is never trusted.
        (Some("/etc"), false),
        (Some("/workspaces"), false),
    ];
    for (worktree, removed) in cases {
        let dir = tempfile::tempdir().unwrap();
        let host = FileHost::default();
        host.put("a.txt", b"hello");
        let (svc, host, id) = setup(host, dir.path());
        set_worktree(&svc, id, worktree, removed);
        let file = svc.resolve_cloud_file(id, "a.txt").unwrap();
        assert_eq!(file.cwd, None, "{worktree:?} {removed}");
        assert_eq!(file.root, ROOT);
        svc.open_cloud_file(&file, &AtomicBool::new(false)).unwrap();
        assert_eq!(host.cwds(), vec![None], "{worktree:?} {removed}");
    }
}
