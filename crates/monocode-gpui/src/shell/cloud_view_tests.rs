//! Tests for opening files of a cloud workspace from chat links.
use super::cloud::CloudMark;
use super::cloud_view::{
    self, Cached, Outcome, cached_open, cloud_target, read_for_click, read_for_view,
    takes_cloud_link,
};
use super::*;
use crate::document_lightbox::{Kind, MAX_BYTES};
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{
        CloudBackendKind, CloudSessions, Host,
        view::{CHUNK_BYTES, FileKind, PlanReason},
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
            return Ok(json!({"capabilities": ["files.read", "files.readBytes"]}));
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
        let end = (offset + params["length"].as_u64().unwrap().min(CHUNK_BYTES) as usize)
            .min(bytes.len());
        use base64::Engine;
        Ok(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(&bytes[offset..end]),
            "offset": end,
            "size": bytes.len(),
        }))
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
