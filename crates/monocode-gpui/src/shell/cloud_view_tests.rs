//! Tests for opening files of a cloud workspace from chat links.
use super::cloud::CloudMark;
use super::cloud_view::{
    self, Cached, MAX_QUEUED_OPENS, OpenQueue, Outcome, cached_open, cloud_target, opens_by_itself,
    read_for_click, read_for_view, takes_cloud_link,
};
use super::*;
use crate::document_lightbox::{Kind, MAX_BYTES};
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{
        CloudBackendKind, CloudSessions, Host,
        view::{CHUNK_BYTES, FileKind, HostUpdate, PlanReason},
        view_cache::Downloaded,
    },
    codespaces::{GhOutput, GhRunner},
    remote::Machine,
    store::Store,
};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

/// A scratch folder removed on drop.
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("monoptah-cloud-view-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const ROOT: &str = "/workspaces/widgets";

struct NoGh;
impl GhRunner for NoGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        panic!("viewing a file must not call gh: {args:?}");
    }
}

/// Serves `files.readBytes` from memory; every other call fails the test.
#[derive(Default)]
struct FileHost {
    files: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    log: Mutex<Vec<String>>,
    disconnected: bool,
    /// Whether `ensure` may be called; it brings a disconnected host back.
    may_reconnect: bool,
    reconnected: AtomicBool,
    ensured: Mutex<u32>,
    /// The Host does not advertise `files.readBytes` (until the test flips it).
    legacy: AtomicBool,
    /// What the Host update looks like while `legacy` is set (`None`: nothing recorded).
    update: Mutex<Option<HostUpdate>>,
    /// Report this size for every file (a huge file without storing it).
    fake_size: Option<u64>,
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
        panic!("viewing must not connect");
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        assert!(self.may_reconnect, "viewing must not wake the workspace");
        *self.ensured.lock().unwrap() += 1;
        self.reconnected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn request(&self, _: &str, method: &str, params: Value) -> Result<Value, String> {
        if method == "environment.describe" {
            let legacy = self.legacy.load(std::sync::atomic::Ordering::SeqCst);
            return Ok(json!({"capabilities": if legacy {
                json!(["files.read"])
            } else {
                json!(["files.read", "files.readBytes"])
            }}));
        }
        assert_eq!(method, "files.readBytes");
        let path = params["path"].as_str().unwrap().to_owned();
        self.log.lock().unwrap().push(path.clone());
        if self.disconnected && !self.reconnected.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(monocode_core::remote::CODESPACE_NOT_CONNECTED.into());
        }
        let files = self.files.lock().unwrap();
        let bytes = files.get(&path).ok_or("ENOENT")?;
        let offset = params["offset"].as_u64().unwrap() as usize;
        let size = self.fake_size.map_or(bytes.len(), |size| size as usize);
        let end = (offset + params["length"].as_u64().unwrap().min(CHUNK_BYTES) as usize).min(size);
        use base64::Engine;
        let part = if self.fake_size.is_some() {
            vec![0u8; end - offset]
        } else {
            bytes[offset..end].to_vec()
        };
        Ok(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(part),
            "offset": end,
            "size": size,
        }))
    }
    fn host_update(&self, _: &str) -> HostUpdate {
        self.update
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(HostUpdate::NextConnect)
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        panic!("viewing must not disconnect");
    }
}

fn service(host: FileHost, dir: &std::path::Path) -> (CloudSessions, Arc<FileHost>, i64) {
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
    let id = store
        .lock()
        .unwrap()
        .create("claude", &project.key)
        .unwrap()
        .id;
    (svc, host, id)
}

#[test]
fn clicked_links_become_workspace_paths() {
    assert_eq!(
        cloud_target("/workspaces/widgets/.evidence/a.html"),
        "/workspaces/widgets/.evidence/a.html"
    );
    assert_eq!(cloud_target(".evidence/a.png"), ".evidence/a.png");
    assert_eq!(cloud_target("src/lib.rs:12:3"), "src/lib.rs");
    assert_eq!(cloud_target("src/lib.rs#L9"), "src/lib.rs");
    // The environment of a remote:// link is checked by the core, which has the session.
    assert_eq!(
        cloud_target("remote://env-octo-abc/workspaces/widgets/r.md"),
        "remote://env-octo-abc/workspaces/widgets/r.md"
    );
    assert_eq!(
        cloud_target("file:///workspaces/widgets/my%20file.md"),
        "/workspaces/widgets/my file.md"
    );
}

fn idle_workspace(tag: &str, may_reconnect: bool) -> (Scratch, CloudSessions, Arc<FileHost>, i64) {
    let dir = Scratch::new(tag);
    let host = FileHost {
        disconnected: true,
        may_reconnect,
        ..FileHost::default()
    };
    host.files
        .lock()
        .unwrap()
        .insert("notes.md".into(), b"# Hi".to_vec());
    let (svc, host, id) = service(host, dir.path());
    (dir, svc, host, id)
}

/// A plain read (a background one) of an idle workspace never wakes it: it says to reconnect.
#[test]
fn reading_an_idle_workspace_without_a_click_never_wakes_it() {
    let (_dir, svc, host, id) = idle_workspace("idle-read", false);
    let never = AtomicBool::new(false);
    match read_for_view(&svc, id, "notes.md", &never) {
        Outcome::Message(text) => assert!(text.starts_with("Reconnect to view"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(*host.ensured.lock().unwrap(), 0);
}

/// A click on a file link reconnects the idle workspace, opens the file, says it reconnected
/// and hands the reconnect progress lines to the card.
#[test]
fn clicking_a_link_in_an_idle_workspace_reconnects_then_opens_the_file() {
    let (_dir, svc, host, id) = idle_workspace("idle-click", true);
    let never = AtomicBool::new(false);
    let mut lines = Vec::new();
    let (outcome, reconnected) = read_for_click(
        &svc,
        id,
        "notes.md",
        &never,
        &|progress| {
            progress("Starting the Host");
            host.ensure("env-octo-abc", &never, progress)?;
            Ok(true)
        },
        &mut |line| lines.push(line.to_owned()),
    );
    match outcome {
        Outcome::Document { doc, .. } => assert_eq!(doc.text.as_deref(), Some("# Hi")),
        other => panic!("{other:?}"),
    }
    assert!(reconnected);
    assert_eq!(lines, ["Starting the Host"]);
    assert_eq!(*host.ensured.lock().unwrap(), 1);
}

/// A connected workspace is read at once: no reconnect and nothing to report.
#[test]
fn clicking_a_link_in_a_connected_workspace_does_not_reconnect() {
    let dir = Scratch::new("connected-click");
    let host = FileHost::default();
    host.files
        .lock()
        .unwrap()
        .insert("notes.md".into(), b"# Hi".to_vec());
    let (svc, _host, id) = service(host, dir.path());
    let never = AtomicBool::new(false);
    let (outcome, reconnected) = read_for_click(
        &svc,
        id,
        "notes.md",
        &never,
        &|_| panic!("a connected workspace is not reconnected"),
        &mut |_| {},
    );
    assert!(matches!(outcome, Outcome::Document { .. }), "{outcome:?}");
    assert!(!reconnected);
}

/// Every way a click reconnect can end without a file shows a message on the card.
#[test]
fn a_click_reconnect_that_does_not_happen_or_fails_shows_why() {
    let (_dir, svc, _host, id) = idle_workspace("click-fails", false);
    let never = AtomicBool::new(false);
    let click = |reconnect: &cloud_view::Reconnect<'_>| {
        let (outcome, reconnected) =
            read_for_click(&svc, id, "notes.md", &never, reconnect, &mut |_| {});
        assert!(!reconnected);
        match outcome {
            Outcome::Message(text) => text,
            other => panic!("{other:?}"),
        }
    };
    // Unavailable (or local): nothing to reconnect, the normal "Reconnect to view" answer.
    assert!(click(&|_| Ok(false)).starts_with("Reconnect to view"));
    // The reconnect fails: its message is shown.
    assert_eq!(
        click(&|_| Err("Sandbox was deleted. Start a new session.".into())),
        "Sandbox was deleted. Start a new session."
    );
    // Another reconnect of the same sandbox is running.
    assert_eq!(
        click(&|_| Err("This sandbox is already reconnecting.".into())),
        "A reconnect of this sandbox is already in progress. Try again in a moment."
    );
    // Cancelled by the user: the card just closes.
    let cancelled = AtomicBool::new(true);
    let (outcome, reconnected) = read_for_click(
        &svc,
        id,
        "notes.md",
        &cancelled,
        &|_| Err("Cancelled".into()),
        &mut |_| {},
    );
    assert_eq!(outcome, Outcome::Message(String::new()));
    assert!(!reconnected);
}

/// A Docker sandbox is reconnected through the shared service's sandbox path (the one the
/// Reconnect button uses), never the codespace wake. This fake service has no sandbox backend,
/// so the click says so on the card and touches neither `gh` nor the host.
#[gpui::test]
fn clicking_a_link_in_a_stopped_sandbox_uses_the_sandbox_reconnect_path(cx: &mut TestAppContext) {
    use crate::cloud_wake::fakes::{AUTH_OK, github_project, install};
    let _ = cx;
    let repo = github_project();
    // The file service is disconnected, so the read asks for a reconnect.
    let (_dir, svc, _file_host, file_id) = idle_workspace("sandbox-click", false);
    let store: Database = Arc::new(Mutex::new(
        monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let (gh, host) = install(store.clone(), AUTH_OK, "Shutdown");
    let id = {
        let guard = store.lock().unwrap();
        let project = guard
            .remember_cloud_ref_project(
                "env-box",
                &json!({"id":"project-1","cwd":ROOT}),
                repo.0.to_str().unwrap(),
                &monocode_core::cloud_sessions::CloudRef::sandbox("monoptah-fix-1234"),
            )
            .unwrap();
        guard.create("claude", &project.key).unwrap().id
    };
    host.0.lock().unwrap().clear();
    gh.calls.lock().unwrap().clear();
    let never = AtomicBool::new(false);
    let (outcome, reconnected) = read_for_click(
        &svc,
        file_id,
        "notes.md",
        &never,
        &|progress| crate::cloud_wake::reconnect_blocking(&store, id, &never, progress),
        &mut |_| {},
    );
    assert!(!reconnected);
    assert_eq!(
        outcome,
        Outcome::Message("Docker sandboxes are not available.".into())
    );
    assert!(host.0.lock().unwrap().is_empty());
    assert!(gh.calls.lock().unwrap().is_empty());
}

/// The tests of the copy-free open: small files in memory, big ones streamed then opened, the
/// huge ones confirmed, and a Host without `files.readBytes` queued or explained.
fn sized_service(
    tag: &str,
    file: &str,
    bytes: Vec<u8>,
    fake_size: Option<u64>,
) -> (Scratch, CloudSessions, i64) {
    let dir = Scratch::new(tag);
    let host = FileHost {
        fake_size,
        ..FileHost::default()
    };
    host.files.lock().unwrap().insert(file.into(), bytes);
    let (svc, _host, id) = service(host, dir.path());
    (dir, svc, id)
}

#[test]
fn a_small_image_opens_in_memory_without_touching_the_disk() {
    let (dir, svc, id) = sized_service("small-image", "shot.png", vec![1, 2, 3], None);
    let never = AtomicBool::new(false);
    assert_eq!(
        read_for_view(&svc, id, "shot.png", &never),
        Outcome::Image {
            data: "AQID".into(),
            mime: "image/png",
            name: "shot.png".into()
        }
    );
    assert!(!dir.path().join("sessions.db.threads").exists());
}

#[test]
fn a_20_mb_pdf_streams_with_progress_then_opens_by_itself() {
    let size = 20 * 1024 * 1024;
    let (_dir, svc, id) = sized_service("pdf", "report.pdf", vec![7; size], None);
    let never = AtomicBool::new(false);
    let Outcome::Download { file, plan } = read_for_view(&svc, id, "report.pdf", &never) else {
        panic!("a PDF streams to the cache");
    };
    // Under the threshold: no question, the download just starts.
    assert!(!plan.needs_confirm);
    assert_eq!(plan.size, size as u64);
    let mut seen: Vec<(u64, u64)> = Vec::new();
    let done = svc
        .download_cloud_file(&file, false, &never, &mut |done, total| {
            seen.push((done, total))
        })
        .unwrap();
    assert_eq!(done.size, size as u64);
    assert_eq!(std::fs::metadata(&done.path).unwrap().len(), size as u64);
    assert!(
        seen.len() > 10,
        "progress arrives chunk by chunk: {}",
        seen.len()
    );
    assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "never goes back");
    assert_eq!(seen.last(), Some(&(size as u64, size as u64)));
    // Nothing to click: a PDF goes straight to the default app.
    assert!(matches!(cached_open(&done), Cached::External { .. }));
    assert!(opens_by_itself(&done.path));
    // Programs and installers are only revealed.
    assert!(!opens_by_itself(std::path::Path::new("/c/setup.pkg")));
    assert!(!opens_by_itself(std::path::Path::new("/c/run.sh")));
}

#[test]
fn a_600_mb_file_asks_before_anything_is_downloaded() {
    let size = 600 * 1024 * 1024;
    let (dir, svc, id) = sized_service("big", "dump.bin", vec![0], Some(size));
    let never = AtomicBool::new(false);
    let Outcome::Download { file, plan } = read_for_view(&svc, id, "dump.bin", &never) else {
        panic!("a big file is a download");
    };
    assert!(plan.needs_confirm);
    assert_eq!(plan.size, size);
    assert_eq!(
        svc.download_cloud_file(&file, false, &never, &mut |_, _| {})
            .unwrap_err(),
        monocode_core::cloud_sessions::view::ViewError::NeedsConfirm { size }
    );
    assert!(
        !dir.path()
            .join("sessions.db.threads")
            .join("1")
            .join("cloud-cache")
            .join("dump.bin")
            .exists(),
        "nothing was stored before the answer"
    );
}

fn legacy_host(update: Option<HostUpdate>) -> FileHost {
    let host = FileHost {
        legacy: AtomicBool::new(true),
        update: Mutex::new(update),
        ..FileHost::default()
    };
    host.files
        .lock()
        .unwrap()
        .insert("shot.png".into(), vec![1, 2, 3]);
    host
}

#[test]
fn a_host_without_read_bytes_says_the_file_opens_after_the_update_and_queues_it() {
    let never = AtomicBool::new(false);
    for update in [Some(HostUpdate::Pending), None] {
        let dir = Scratch::new("legacy-pending");
        let (svc, _host, id) = service(legacy_host(update), dir.path());
        match read_for_view(&svc, id, "shot.png", &never) {
            Outcome::HostUpdate { text, queue } => {
                assert!(queue);
                assert!(
                    text.starts_with("shot.png opens after the Host update"),
                    "{text}"
                );
                assert!(
                    text.contains("next time this workspace connects")
                        || text.contains("updated on the next connect"),
                    "{text}"
                );
                assert!(!text.to_lowercase().contains("download"), "{text}");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_blocked_host_update_shows_the_policy_hint_and_queues_nothing() {
    let dir = Scratch::new("legacy-blocked");
    let warning = monocode_core::remote::host_upgrade::warning(
        "curl: (6) Could not resolve host: github.com",
        true,
    );
    let (svc, _host, id) = service(legacy_host(Some(HostUpdate::Failed(warning))), dir.path());
    let never = AtomicBool::new(false);
    match read_for_view(&svc, id, "shot.png", &never) {
        Outcome::HostUpdate { text, queue } => {
            assert!(!queue, "a blocked update does not fix itself");
            assert!(
                text.contains("Monoptah Host could not be updated"),
                "{text}"
            );
            assert!(
                text.contains("blocked by the sandbox network policy"),
                "{text}"
            );
            assert!(
                text.contains(monocode_core::remote::host_upgrade::POLICY_HINT),
                "{text}"
            );
            assert!(!text.contains("Download"), "{text}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_open_queue_keeps_the_latest_click_per_session_bounded_and_cleared_on_a_switch() {
    let mut queue = OpenQueue::default();
    queue.push(1, "a.png".into());
    queue.push(1, "b.png".into());
    assert_eq!(queue.len(), 1, "one entry per session");
    for n in 0..MAX_QUEUED_OPENS + 3 {
        queue.push(100 + n as i64, "f.png".into());
    }
    assert_eq!(queue.len(), MAX_QUEUED_OPENS);
    assert!(!queue.has(1), "the oldest session went");
    queue.push(2, "other.png".into());
    assert!(queue.has(2));
    queue.keep_only(Some(2));
    assert!(!queue.has(100 + MAX_QUEUED_OPENS as i64));
    assert_eq!(queue.take(2).as_deref(), Some("other.png"));
    assert_eq!(queue.take(2), None);
    queue.push(3, "x".into());
    queue.keep_only(None);
    assert_eq!(queue.len(), 0);
}

fn mark(shell: &mut Shell, id: i64) {
    // The background refresh forgets the fake mark (the store has no such codespace).
    shell.cloud_marks.insert(
        id,
        CloudMark {
            local_project: "/Users/me/widgets".into(),
            codespace: "octo-abc".into(),
            kind: CloudBackendKind::Codespaces,
        },
    );
}

/// A queued open that has to wait again stays quiet and a "not connected" answer is not shown.
#[test]
fn a_queued_read_that_is_not_connected_stays_quiet_and_waits() {
    let (_dir, svc, host, id) = idle_workspace("queued-idle", false);
    let never = AtomicBool::new(false);
    match cloud_view::read_queued(&svc, id, "notes.md", &never) {
        Outcome::HostUpdate { text, queue } => {
            assert!(queue);
            assert!(text.starts_with("Reconnect to view"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(*host.ensured.lock().unwrap(), 0, "nothing was woken");
}

/// The whole path in the shell, through the render: the click is queued while the Host is old
/// (its message card stays on screen), a connect that leaves the Host old changes nothing, and
/// the connect that finishes the update opens the file by itself, without any wake.
#[gpui::test]
fn a_queued_open_runs_after_the_connect_that_finishes_the_update(cx: &mut TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let dir = Scratch::new("queued-shell");
    let host = legacy_host(Some(HostUpdate::Pending));
    let (svc, host, id) = service(host, dir.path());
    window
        .update(cx, |shell, w, cx| {
            shell.cloud = Arc::new(svc);
            shell.workspace.active = Some(id);
            mark(shell, id);
            assert!(shell.open_cloud_path("shot.png", w, cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let text = shell.cloud_view_text();
            assert!(text.contains("opens after the Host update"), "{text}");
            assert!(shell.queued_cloud_opens.has(id));
            // The card is still on screen at the next connect (no dismiss); the Host is still old.
            mark(shell, id);
        })
        .unwrap();
    window
        .update(cx, |shell, _, cx| shell.cloud_resumed(id, cx))
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(
                shell.cloud_view.is_none(),
                "a queued retry replaces the card quietly"
            );
            assert!(
                shell.queued_cloud_opens.has(id),
                "and waits for the next connect"
            );
            assert_eq!(shell.cloud_open_due, None);
            mark(shell, id);
        })
        .unwrap();
    // A later connect installed the new Host.
    host.legacy
        .store(false, std::sync::atomic::Ordering::SeqCst);
    window
        .update(cx, |shell, _, cx| shell.cloud_resumed(id, cx))
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(
                shell.cloud_view.is_none(),
                "the image was handed to its viewer"
            );
            assert!(!shell.queued_cloud_opens.has(id));
        })
        .unwrap();
    let reads = host.log.lock().unwrap().clone();
    assert!(reads.iter().any(|p| p == "shot.png"), "{reads:?}");
    assert_eq!(*host.ensured.lock().unwrap(), 0, "nothing was woken");
}

/// A click on a PDF streams it to the cache and the shell hands it to the default app; a
/// program is only revealed.
#[gpui::test]
fn the_shell_opens_a_streamed_pdf_by_itself_and_only_reveals_a_program(cx: &mut TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let dir = Scratch::new("pdf-shell");
    let host = FileHost::default();
    host.files
        .lock()
        .unwrap()
        .insert("out/report.pdf".into(), vec![7; 1024 * 1024]);
    host.files
        .lock()
        .unwrap()
        .insert("setup.pkg".into(), vec![1; 10]);
    let (svc, _host, id) = service(host, dir.path());
    cloud_view::OPENED.lock().unwrap().clear();
    window
        .update(cx, |shell, w, cx| {
            shell.cloud = Arc::new(svc);
            shell.workspace.active = Some(id);
            mark(shell, id);
            assert!(shell.open_cloud_path("out/report.pdf", w, cx));
        })
        .unwrap();
    cx.run_until_parked();
    {
        let opened = cloud_view::OPENED.lock().unwrap();
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert!(
            opened[0].ends_with("cloud-cache/out/report.pdf"),
            "{opened:?}"
        );
        assert_eq!(std::fs::metadata(&opened[0]).unwrap().len(), 1024 * 1024);
    }
    window
        .update(cx, |shell, w, cx| {
            assert!(shell.cloud_view.is_none(), "no card is left behind");
            mark(shell, id);
            assert!(shell.open_cloud_path("setup.pkg", w, cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(shell.cloud_view_saved()))
        .unwrap();
    assert_eq!(
        cloud_view::OPENED.lock().unwrap().len(),
        1,
        "the installer was not opened"
    );
}

/// A queued stream that ends after the user switched away is dropped: nothing opens and the
/// downloaded file is deleted.
#[gpui::test]
fn a_queued_open_is_dropped_when_the_session_is_left(cx: &mut TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let dir = Scratch::new("queued-switch");
    let host = FileHost::default();
    host.files
        .lock()
        .unwrap()
        .insert("report.pdf".into(), vec![7; 1024 * 1024]);
    let (svc, _host, id) = service(host, dir.path());
    let cache = svc
        .resolve_cloud_file(id, "report.pdf")
        .unwrap()
        .cache_dir
        .unwrap();
    cloud_view::OPENED.lock().unwrap().clear();
    window
        .update(cx, |shell, w, cx| {
            shell.cloud = Arc::new(svc);
            shell.workspace.active = Some(id);
            mark(shell, id);
            shell.queued_cloud_opens.push(id, "report.pdf".into());
            shell.flush_queued_cloud_opens(id, w, cx);
            assert!(shell.cloud_view.is_some());
            // The user goes to a Mono or terminal pane (not a cloud session) before it ends.
            shell.leave_queued_cloud_opens(None, cx);
            assert!(shell.cloud_view.is_none(), "the queued read was cancelled");
            assert!(!shell.queued_cloud_opens.has(id));
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cloud_view::OPENED.lock().unwrap().is_empty());
    assert!(!cache.join("report.pdf").exists());
}

#[gpui::test]
fn switching_sessions_drops_the_queued_opens_of_the_other_one(cx: &mut TestAppContext) {
    let (window, a, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.queued_cloud_opens.push(a + 1000, "x.png".into());
            shell.cloud_open_due = Some(a + 1000);
            shell.activate_session(a, w, cx);
            assert!(!shell.queued_cloud_opens.has(a + 1000));
            assert_eq!(shell.cloud_open_due, None);
        })
        .unwrap();
}

#[test]
fn files_that_were_read_in_memory_become_viewer_actions() {
    let dir = Scratch::new("open");
    let host = FileHost::default();
    host.files
        .lock()
        .unwrap()
        .insert(".evidence/shot.png".into(), vec![1, 2, 3]);
    host.files
        .lock()
        .unwrap()
        .insert("notes.md".into(), b"# Hi".to_vec());
    host.files
        .lock()
        .unwrap()
        .insert("src/a.rs".into(), b"fn a() {}".to_vec());
    host.files
        .lock()
        .unwrap()
        .insert("r.html".into(), b"<b>x</b>".to_vec());
    host.files
        .lock()
        .unwrap()
        .insert("big.png".into(), vec![0; MAX_BYTES as usize + 1]);
    let (svc, _host, id) = service(host, dir.path());
    let never = AtomicBool::new(false);
    let read = |raw: &str| read_for_view(&svc, id, raw, &never);
    assert_eq!(
        read("/workspaces/widgets/.evidence/shot.png"),
        Outcome::Image {
            data: "AQID".into(),
            mime: "image/png",
            name: "shot.png".into()
        }
    );
    match read("notes.md") {
        Outcome::Document { name, path, doc } => {
            assert_eq!(
                (name.as_str(), path.to_str()),
                ("notes.md", Some("notes.md"))
            );
            assert_eq!(doc.kind, Kind::Markdown);
            assert_eq!(doc.text.as_deref(), Some("# Hi"));
            assert!(doc.web_root.is_none());
        }
        other => panic!("{other:?}"),
    }
    match read("src/a.rs") {
        Outcome::Document { doc, .. } => assert_eq!(doc.kind, Kind::Code),
        other => panic!("{other:?}"),
    }
    match read("r.html") {
        Outcome::Document { doc, .. } => {
            assert_eq!(doc.kind, Kind::Html);
            assert_eq!(doc.text.as_deref(), Some("<b>x</b>"));
        }
        other => panic!("{other:?}"),
    }
    match read("big.png") {
        Outcome::Download { plan, .. } => assert_eq!(plan.reason, PlanReason::TooLarge),
        other => panic!("{other:?}"),
    }
    assert!(
        !dir.path().join("sessions.db.threads").exists(),
        "nothing touched the disk"
    );
}

#[test]
fn unreachable_and_outside_files_say_so_without_waking_anything() {
    let dir = Scratch::new("gone");
    let host = FileHost {
        disconnected: true,
        ..Default::default()
    };
    host.files.lock().unwrap().insert("a.png".into(), vec![1]);
    let (svc, host, id) = service(host, dir.path());
    let never = AtomicBool::new(false);
    assert_eq!(
        read_for_view(&svc, id, "a.png", &never),
        Outcome::Message("Reconnect to view a.png".into())
    );
    assert_eq!(host.log.lock().unwrap().len(), 1);
    match read_for_view(&svc, id, "/etc/passwd", &never) {
        Outcome::Message(text) => assert!(text.contains("outside the cloud workspace"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        host.log.lock().unwrap().len(),
        1,
        "the refusal sent no request"
    );
}

#[test]
fn downloaded_files_open_in_the_right_viewer_or_are_left_to_the_default_app() {
    let done = |kind, size| Downloaded {
        kind,
        path: "/c/cloud-cache/a/f".into(),
        cache_dir: "/c/cloud-cache".into(),
        size,
    };
    assert_eq!(
        cached_open(&done(FileKind::Html, 9 << 20)),
        Cached::Html {
            path: "/c/cloud-cache/a/f".into(),
            root: "/c/cloud-cache".into()
        }
    );
    assert_eq!(
        cached_open(&done(FileKind::Image, 90 << 20)),
        Cached::Image {
            path: "/c/cloud-cache/a/f".into()
        }
    );
    assert_eq!(
        cached_open(&done(FileKind::Markdown, MAX_BYTES)),
        Cached::Document {
            path: "/c/cloud-cache/a/f".into(),
            kind: Kind::Markdown
        }
    );
    assert_eq!(
        cached_open(&done(FileKind::Text, 10)),
        Cached::Document {
            path: "/c/cloud-cache/a/f".into(),
            kind: Kind::Code
        }
    );
    for (kind, size) in [(FileKind::Text, MAX_BYTES + 1), (FileKind::Other, 5)] {
        assert!(matches!(
            cached_open(&done(kind, size)),
            Cached::External { .. }
        ));
    }
}

#[gpui::test]
fn only_cloud_sessions_take_over_file_links(cx: &mut TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let scratch = Scratch::new("local");
    let file = scratch.path().join("local.md");
    std::fs::write(&file, "x").unwrap();
    let local = file.to_string_lossy().into_owned();
    window
        .update(cx, |shell, window, cx| {
            let id = shell.workspace.active.unwrap_or(7);
            // A local session: the card never appears and the link is left to the editor.
            assert!(!shell.is_cloud(id));
            assert!(!shell.open_cloud_path("/workspaces/widgets/a.md", window, cx));
            assert!(shell.cloud_view.is_none());
            shell.workspace.active = Some(id);
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            // A real local file keeps today's behaviour even in a cloud session.
            assert!(!shell.open_cloud_path(&local, window, cx));
            assert!(shell.cloud_view.is_none());
            // Anything else is read from the cloud workspace: the card shows at once.
            assert!(shell.open_cloud_path("/workspaces/widgets/a.md", window, cx));
            assert!(shell.cloud_view.is_some());
            shell.dismiss_cloud_view(cx);
            assert!(shell.cloud_view.is_none());
        })
        .unwrap();
}

#[test]
fn a_mono_over_the_session_or_the_document_lightbox_keeps_its_own_links() {
    assert!(takes_cloud_link(false, false));
    assert!(!takes_cloud_link(true, false));
    assert!(!takes_cloud_link(false, true));
    assert!(!takes_cloud_link(true, true));
}

#[gpui::test]
fn a_click_in_the_document_lightbox_is_never_read_from_the_cloud(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::input::bind(cx);
        crate::document_lightbox::bind(cx);
        super::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let (window, _, _) = crate::shell::tests::window(cx);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut visual, |shell, w, cx| {
            let id = shell.workspace.active.unwrap_or(7);
            shell.workspace.active = Some(id);
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            shell.focus_active(w, cx);
            w.dispatch_action(
                Box::new(crate::document_lightbox::Open {
                    path: "/nonexistent/doc.md".into(),
                    title: "doc.md".into(),
                    cloud: Some(crate::document_lightbox::CloudDoc {
                        kind: Kind::Markdown,
                        text: Some("[next](next.md)".into()),
                        web_root: None,
                    }),
                }),
                cx,
            );
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |shell, w, cx| {
            assert!(shell.document_lightbox.is_some());
            assert!(!shell.open_cloud_path("next.md", w, cx));
            assert!(shell.cloud_view.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn archiving_a_cloud_session_stops_its_card_and_deletes_its_cache(cx: &mut TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let dir = Scratch::new("archive");
    let (svc, _host, id) = service(FileHost::default(), dir.path());
    let cache = svc
        .resolve_cloud_file(id, "a.bin")
        .unwrap()
        .cache_dir
        .unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("a.bin"), "x").unwrap();
    let evidence = cache.parent().unwrap().join("evidence");
    std::fs::create_dir_all(&evidence).unwrap();
    window
        .update(cx, |shell, w, cx| {
            shell.cloud = Arc::new(svc);
            shell.workspace.active = Some(id);
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/Users/me/widgets".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            // A read in flight for this session.
            assert!(shell.open_cloud_path("a.bin", w, cx));
            assert!(shell.cloud_view.is_some());
            shell.finish_archive(&[id], &[id], true, true, w, cx);
            assert!(
                shell.cloud_view.is_none(),
                "the card for the archived session is gone"
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!cache.exists(), "the cache is deleted");
    assert!(evidence.exists(), "saved evidence is kept");
}
