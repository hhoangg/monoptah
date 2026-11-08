//! View-model and flow tests for cloud sessions, with a fake `gh` and a fake host.
use super::cloud::{self, ArchiveChoice, CloudMark, Then, Tone};
use super::cloud_picker;
use super::*;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{
        CloudBackendKind, CloudRef, CloudSessions, CloudState, CodespaceChoice, ConvertError,
        Converted, Host, Resume,
    },
    codespaces::{GhOutput, GhRunner},
    remote::Machine,
    store::Store,
};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

const AUTH_OK: &str = "github.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const LIST: &str =
    r#"[{"name":"octo-abc","displayName":"my cs","state":"Shutdown","repository":"octo/widgets"}]"#;

/// Replies by the first rule whose text is in the joined argv and records every call.
struct FakeGh {
    rules: Vec<(&'static str, &'static str)>,
    calls: Mutex<Vec<String>>,
}
impl FakeGh {
    fn new(rules: Vec<(&'static str, &'static str)>) -> Arc<Self> {
        Arc::new(Self {
            rules,
            calls: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// Existing archive safety assertions concern lifecycle writes, separately
    /// from the read-only auth/inventory preflight covered below.
    fn archive_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| {
                call != "auth status --active" && call != "api --paginate --slurp user/codespaces"
            })
            .collect()
    }
}
impl GhRunner for FakeGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let stdout = self
            .rules
            .iter()
            .find(|(key, _)| line.contains(key))
            .map(|(_, out)| *out)
            .unwrap_or_else(|| match line.as_str() {
                "auth status --active" => AUTH_OK,
                "api --paginate --slurp user/codespaces" => {
                    r#"[{"total_count":2,"codespaces":[{"name":"octo-abc"},{"name":"octo-xyz"}]}]"#
                }
                _ => "",
            });
        Ok(GhOutput {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[derive(Default)]
struct FakeHost {
    machines: Mutex<Vec<Machine>>,
    log: Mutex<Vec<String>>,
}
fn machine(codespace: &str) -> Machine {
    Machine {
        id: format!("m-{codespace}"),
        name: codespace.into(),
        endpoint: format!("codespace://{codespace}"),
        environment_id: format!("env-{codespace}"),
        ssh: None,
        codespace: Some(codespace.into()),
        sandbox: None,
    }
}
impl Host for FakeHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(self.machines.lock().unwrap().clone())
    }
    fn connect(
        &self,
        codespace: &str,
        _: &str,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        progress("Pairing with the codespace");
        self.log
            .lock()
            .unwrap()
            .push(format!("connect {codespace}"));
        let m = machine(codespace);
        self.machines.lock().unwrap().push(m.clone());
        Ok(m)
    }
    fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("ensure {id}"));
        Ok(())
    }
    fn request(&self, _: &str, method: &str, params: Value) -> Result<Value, String> {
        self.log.lock().unwrap().push(method.to_string());
        Ok(json!({"id":"project-1","cwd":params["cwd"]}))
    }
    fn disconnect(&self, id: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("disconnect {id}"));
        Ok(())
    }
}

fn service(gh: &Arc<FakeGh>, host: &Arc<FakeHost>, store: Database) -> CloudSessions {
    CloudSessions::new(gh.clone(), host.clone(), store)
}

fn memory_store() -> Database {
    Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ))
}

/// A real git repository whose origin is a GitHub repository; removed on drop.
struct Checkout(std::path::PathBuf);
impl Checkout {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn github_checkout() -> Checkout {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "monoptah-cloud-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["remote", "add", "origin", "git@github.com:octo/widgets.git"],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
    Checkout(dir)
}

/// A conversion that behaves like the real one on the store, or fails with `fail`.
fn fake_convert(store: Database, fail: Option<&'static str>) -> cloud::ConvertFn {
    Arc::new(move |_, id, _files, progress| {
        progress("Waking the codespace");
        if let Some(error) = fail {
            return Err(ConvertError::Failed(error.into()));
        }
        let s = store.lock().unwrap();
        let remote = s.remote_session(id).unwrap().unwrap();
        let local = s
            .create("claude", remote.project.local_project.as_deref().unwrap())
            .unwrap();
        s.set_archived(local.id, true).unwrap();
        s.forget_cloud_session(id).unwrap();
        let summary = s
            .sessions()
            .unwrap()
            .into_iter()
            .find(|x| x.id == local.id)
            .unwrap();
        Ok(Converted {
            cloud_id: id,
            local_id: local.id,
            title: summary.title.clone(),
            summary,
            codespace: remote.project.cloud().unwrap().name,
            rows: 0,
            resume: Resume::Missing("test".into()),
            evidence: vec![],
            code_dir: None,
            ignored: vec![],
            ignored_truncated: false,
            evidence_partial: false,
            warnings: vec![],
            history_saved: true,
        })
    })
}

/// Archived local sessions of `/work/monocode` the shell knows.
fn archived_locals(shell: &Shell) -> Vec<i64> {
    let mut ids: Vec<i64> = shell
        .session_summaries
        .values()
        .filter(|s| s.archived && s.cwd == "/work/monocode")
        .map(|s| s.id)
        .collect();
    ids.sort();
    ids
}

/// Stores a cloud session of `local` in `codespace` and returns its id.
fn add_cloud_session(store: &Database, local: &str, codespace: &str) -> i64 {
    let store = store.lock().unwrap();
    let project = store
        .remember_cloud_project(
            &format!("env-{codespace}"),
            &json!({"id":"p","cwd":"/workspaces/widgets"}),
            local,
            codespace,
        )
        .unwrap();
    let mut summary = store.create("claude", &project.key).unwrap();
    summary.has_messages = true;
    summary.id
}

#[test]
fn every_state_has_a_row_indicator_and_unreadable_is_not_connected() {
    let cases = [
        (Some(CloudState::Available), "Available", Tone::Good),
        (Some(CloudState::Starting), "Starting", Tone::Busy),
        (Some(CloudState::Stopping), "Stopping", Tone::Busy),
        (Some(CloudState::Stopped), "Stopped", Tone::Idle),
        (Some(CloudState::Unavailable), "Unavailable", Tone::Bad),
        (Some(CloudState::Unknown), "Not connected", Tone::Idle),
        (None, "Not connected", Tone::Idle),
    ];
    for (state, label, tone) in cases {
        let indicator = cloud::indicator(state);
        assert_eq!((indicator.label, indicator.tone), (label, tone));
    }
}

#[test]
fn archiving_defaults_to_stop_unless_other_sessions_use_the_codespace() {
    assert_eq!(cloud::default_choice(0), ArchiveChoice::Stop);
    assert_eq!(cloud::default_choice(1), ArchiveChoice::Keep);
    assert_eq!(cloud::default_choice(4), ArchiveChoice::Keep);
    assert_eq!(cloud::others_note(0, "codespace"), None);
    assert_eq!(
        cloud::others_note(1, "codespace").as_deref(),
        Some("1 other session uses this codespace.")
    );
    assert_eq!(
        cloud::others_note(3, "codespace").as_deref(),
        Some("3 other sessions use this codespace.")
    );
}

#[test]
fn archive_choices_make_the_matching_codespace_calls() {
    let host = Arc::new(FakeHost::default());
    host.machines.lock().unwrap().push(machine("octo-abc"));
    let run = |choice| {
        let gh = FakeGh::new(vec![]);
        let store = memory_store();
        let svc = service(&gh, &host, store);
        let then = cloud::apply_choice(&svc, choice, &CloudRef::codespace("octo-abc")).unwrap();
        (then, gh.calls())
    };
    let (then, calls) = run(ArchiveChoice::Keep);
    assert_eq!((then, calls), ((Then::Archive, None), vec![]));
    let (then, calls) = run(ArchiveChoice::Stop);
    assert_eq!(then, (Then::Archive, None));
    assert_eq!(calls, vec!["codespace stop -c octo-abc".to_string()]);
    let (then, calls) = run(ArchiveChoice::Delete);
    assert_eq!(then, (Then::Forget, None));
    assert_eq!(
        calls,
        vec!["codespace delete -c octo-abc --force".to_string()]
    );
    assert!(
        host.log
            .lock()
            .unwrap()
            .contains(&"disconnect m-octo-abc".to_string())
    );
}

#[test]
fn a_failed_codespace_call_is_reported_and_does_not_archive() {
    struct Failing;
    impl GhRunner for Failing {
        fn run(&self, _: &[String], _: Option<&str>) -> Result<GhOutput, String> {
            Ok(GhOutput {
                success: false,
                stdout: String::new(),
                stderr: "HTTP 500".into(),
            })
        }
    }
    let svc = CloudSessions::new(
        Arc::new(Failing),
        Arc::new(FakeHost::default()),
        memory_store(),
    );
    let error = cloud::apply_choice(&svc, ArchiveChoice::Stop, &CloudRef::codespace("octo-abc"))
        .unwrap_err();
    assert!(
        error.starts_with("Could not stop the codespace:"),
        "{error}"
    );
}

#[test]
fn the_terminal_runs_gh_codespace_ssh_for_the_codespace() {
    let args: Vec<String> = cloud::codespace_ssh_args("octo-abc")
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["codespace", "ssh", "-c", "octo-abc"]);
}

#[test]
fn collect_lists_cloud_sessions_with_cached_state_and_never_wakes() {
    let store = memory_store();
    let id = add_cloud_session(&store, "/work/monocode", "octo-abc");
    add_cloud_session(&store, "/work/other", "octo-xyz");
    let gh = FakeGh::new(vec![("codespace view", r#"{"state":"Shutdown"}"#)]);
    let svc = service(&gh, &Arc::new(FakeHost::default()), store);
    let projects = vec!["/work/monocode".to_string()];
    let snapshot = cloud::collect(&svc, &projects, &[]);
    assert_eq!(
        snapshot.marks.get(&id),
        Some(&CloudMark {
            local_project: "/work/monocode".into(),
            codespace: "octo-abc".into(),
            kind: CloudBackendKind::Codespaces,
        })
    );
    assert_eq!(snapshot.states["octo-abc"], CloudState::Stopped);
    assert_eq!(snapshot.marks.len(), 1);
    // Only a read-only view call: a refresh must never start or stop anything.
    assert_eq!(gh.calls(), ["codespace view -c octo-abc --json state"]);
    // The second refresh is served from the cache.
    cloud::collect(&svc, &projects, &[]);
    assert_eq!(gh.calls().len(), 1);
}

#[test]
fn the_new_session_flow_begins_a_start_and_lists_a_pending_claude_session() {
    let checkout = github_checkout();
    let local = checkout.path().to_string_lossy().into_owned();
    let gh = FakeGh::new(vec![
        ("auth status", AUTH_OK),
        ("codespace list", LIST),
        (
            "--method POST",
            r#"{"name":"octo-new1","state":"Provisioning"}"#,
        ),
        ("codespace view", r#"{"state":"Available"}"#),
    ]);
    let host = Arc::new(FakeHost::default());
    let store = memory_store();
    let svc = service(&gh, &host, store.clone());
    let target =
        cloud_picker::target_for(CodespaceChoice::New, " feature ", Some("basicLinux32gb"))
            .unwrap();
    assert_eq!(target.branch.as_deref(), Some("feature"));
    let (summary, record) = cloud_picker::begin_cloud_start(&svc, &local, target).unwrap();
    assert_eq!(summary.provider, "claude");
    // The session exists at once, on a provisional project, with its start recorded.
    assert!(
        summary.cwd.starts_with("remote://pending-octo-new1"),
        "{}",
        summary.cwd
    );
    assert_eq!(record.codespace, "octo-new1");
    assert_eq!(record.session_id, summary.id);
    assert!(
        gh.calls()
            .iter()
            .any(|c| c.contains("ref=feature") && c.contains("machine=basicLinux32gb"))
    );
    let bound = svc.cloud_sessions_for(&local).unwrap();
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].summary.id, summary.id);
}

#[test]
fn an_existing_codespace_is_used_without_creating_and_a_new_one_needs_a_branch() {
    let checkout = github_checkout();
    let local = checkout.path().to_string_lossy().into_owned();
    let gh = FakeGh::new(vec![("auth status", AUTH_OK), ("codespace list", LIST)]);
    let host = Arc::new(FakeHost::default());
    host.machines.lock().unwrap().push(machine("octo-abc"));
    let svc = service(&gh, &host, memory_store());
    let target =
        cloud_picker::target_for(CodespaceChoice::Named("octo-abc".into()), "", None).unwrap();
    cloud_picker::begin_cloud_start(&svc, &local, target).unwrap();
    assert!(!gh.calls().iter().any(|c| c.contains("--method POST")));
    assert_eq!(
        cloud_picker::target_for(CodespaceChoice::New, "  ", None).unwrap_err(),
        "Enter a branch for the new codespace."
    );
}

fn shell_with_cloud(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, i64, Arc<FakeGh>) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let gh = FakeGh::new(vec![]);
    let id = window
        .update(cx, |shell, _, _| {
            let id = add_cloud_session(&shell.store, "/work/monocode", "octo-abc");
            let summary = locked(&shell.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap();
            shell.cloud = Arc::new(CloudSessions::new(
                gh.clone(),
                Arc::new(FakeHost::default()),
                shell.store.clone(),
            ));
            shell.cloud_convert = fake_convert(shell.store.clone(), None);
            // No conversation entity: a remote one polls its host forever, which
            // would keep the test scheduler from ever parking.
            shell.session_summaries.insert(
                id,
                Session {
                    has_messages: true,
                    ..summary
                },
            );
            shell.order.push(id);
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            shell
                .cloud_states
                .insert("octo-abc".into(), CloudState::Stopped);
            id
        })
        .unwrap();
    (window, id, gh)
}

#[gpui::test]
fn a_cloud_session_is_listed_under_its_local_project_with_its_state(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.workspace.project, "/work/monocode");
            assert!(shell.sidebar_session_ids(cx).contains(&id));
            assert!(shell.cloud_badge(id, false).is_some());
            assert!(shell.cloud_badge(id, true).is_some());
            // A cloud session belongs to its local project, not its remote path.
            let cwd = shell.session_summaries[&id].cwd.clone();
            assert!(cwd.starts_with("remote://"));
            assert_eq!(shell.project_of(id, &cwd), "/work/monocode");
            shell.workspace.active = Some(id);
            assert_eq!(shell.active_cloud(), Some(CloudRef::codespace("octo-abc")));
        })
        .unwrap();
}

#[gpui::test]
fn archiving_a_cloud_session_opens_the_dialog_and_stop_calls_gh_then_archives(
    cx: &mut TestAppContext,
) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| shell.toggle_archive(id, cx))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let dialog = shell.cloud_archive.as_ref().expect("dialog opened");
            assert_eq!(dialog.ids(), vec![id]);
            assert!(!shell.session_summaries[&id].archived, "not archived yet");
        })
        .unwrap();
    assert!(visual.debug_bounds("cloud-archive-stop").is_some());
    assert!(visual.debug_bounds("cloud-archive-delete").is_some());
    assert!(visual.debug_bounds("cloud-archive-keep").is_some());
    let tops: Vec<_> = [
        "cloud-archive-cancel",
        "cloud-archive-keep",
        "cloud-archive-delete",
        "cloud-archive-stop",
    ]
    .into_iter()
    .map(|id| visual.debug_bounds(id).map(|b| b.origin.y))
    .collect();
    assert!(
        tops.iter().all(|top| *top == tops[0]),
        "footer buttons share one row: {tops:?}"
    );
    window
        .update(visual, |shell, window, cx| {
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(gh.calls(), ["codespace stop -c octo-abc"]);
    window
        .update(visual, |shell, _, _| {
            assert!(shell.cloud_archive.is_none());
            // The cloud row is replaced by an archived local copy.
            assert!(!shell.session_summaries.contains_key(&id));
            assert!(!shell.is_cloud(id));
            assert_eq!(archived_locals(shell).len(), 1);
        })
        .unwrap();
}

#[gpui::test]
fn keep_running_archives_without_touching_the_codespace_and_counts_other_sessions(
    cx: &mut TestAppContext,
) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            let other = add_cloud_session(&shell.store, "/work/monocode", "octo-abc");
            let summary = locked(&shell.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == other)
                .unwrap();
            shell.session_summaries.insert(other, summary);
            shell.order.push(other);
            shell.cloud_marks.insert(
                other,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            shell.open_cloud_archive(vec![id], window, cx);
            assert_eq!(shell.cloud_archive.as_ref().unwrap().others(), 1);
            shell.confirm_cloud_archive(ArchiveChoice::Keep, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(gh.calls().is_empty());
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.session_metadata(id, cx).summary.archived)
        })
        .unwrap();
}

#[gpui::test]
fn delete_codespace_removes_its_sessions_from_the_window(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Delete, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert_eq!(gh.archive_calls(), ["codespace delete -c octo-abc --force"]);
    window
        .update(visual, |shell, _, _| {
            assert!(!shell.session_summaries.contains_key(&id));
            assert!(!shell.is_cloud(id));
            // The copy was saved before the codespace went away.
            assert_eq!(archived_locals(shell).len(), 1);
        })
        .unwrap();
}

#[gpui::test]
fn a_failed_copy_leaves_the_codespace_alone_and_offers_retry_anyway_or_cancel(
    cx: &mut TestAppContext,
) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_convert = fake_convert(shell.store.clone(), Some("ssh dropped"));
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Delete, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(
        gh.archive_calls().is_empty(),
        "nothing deleted: {:?}",
        gh.archive_calls()
    );
    window
        .update(visual, |shell, _, _| {
            let dialog = shell.cloud_archive.as_ref().expect("dialog stays open");
            match &dialog.phase {
                cloud::Phase::Failed { choice, error } => {
                    assert_eq!(*choice, ArchiveChoice::Delete);
                    assert!(error.contains("ssh dropped"), "{error}");
                }
                _ => panic!("expected the failed phase"),
            }
            assert!(shell.is_cloud(id) && archived_locals(shell).is_empty());
            assert!(shell.cloud_busy.is_empty());
        })
        .unwrap();
    for id in [
        "cloud-archive-cancel",
        "cloud-archive-anyway",
        "cloud-archive-retry",
    ] {
        assert!(visual.debug_bounds(id).is_some(), "{id}");
    }
    // Retry once the problem is gone: copy first, then delete.
    window
        .update(visual, |shell, window, cx| {
            shell.cloud_convert = fake_convert(shell.store.clone(), None);
            shell.run_cloud_archive(ArchiveChoice::Delete, true, Vec::new(), window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(gh.archive_calls(), ["codespace delete -c octo-abc --force"]);
    window
        .update(visual, |shell, _, _| {
            assert!(shell.cloud_archive.is_none() && !shell.is_cloud(id));
            assert_eq!(archived_locals(shell).len(), 1);
        })
        .unwrap();
}

#[gpui::test]
fn stop_anyway_skips_the_copy_and_archives_the_cloud_session_as_before(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_convert = fake_convert(shell.store.clone(), Some("copy failed"));
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(gh.calls().is_empty());
    window
        .update(visual, |shell, window, cx| {
            shell.run_cloud_archive(ArchiveChoice::Stop, false, Vec::new(), window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(gh.calls(), ["codespace stop -c octo-abc"]);
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.cloud_archive.is_none());
            assert!(shell.session_metadata(id, cx).summary.archived);
            assert!(archived_locals(shell).is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn a_session_converted_by_an_earlier_attempt_is_not_converted_twice(cx: &mut TestAppContext) {
    let (window, a, gh) = shell_with_cloud(cx);
    let b = window
        .update(cx, |shell, _, _| cloud_row(shell, "octo-abc"))
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::<i64>::new()));
    window
        .update(cx, |shell, window, cx| {
            // Converting `b` fails; `a` works.
            let inner = fake_convert(shell.store.clone(), None);
            let log = calls.clone();
            shell.cloud_convert = Arc::new(move |cloud, id, files, progress| {
                log.lock().unwrap().push(id);
                if id == b {
                    Err(ConvertError::Failed("disk full".into()))
                } else {
                    inner(cloud, id, files, progress)
                }
            });
            shell.open_cloud_archive(vec![a, b], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(gh.calls().is_empty());
    window
        .update(visual, |shell, window, cx| {
            // `a` is already a local copy; the dialog only has `b` left.
            assert_eq!(archived_locals(shell).len(), 1);
            assert_eq!(shell.cloud_archive.as_ref().unwrap().ids(), vec![b]);
            let inner = fake_convert(shell.store.clone(), None);
            let log = calls.clone();
            shell.cloud_convert = Arc::new(move |cloud, id, files, progress| {
                log.lock().unwrap().push(id);
                inner(cloud, id, files, progress)
            });
            shell.run_cloud_archive(ArchiveChoice::Stop, true, Vec::new(), window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(*calls.lock().unwrap(), vec![a, b, b]);
    assert_eq!(gh.calls(), ["codespace stop -c octo-abc"]);
    window
        .update(visual, |shell, _, _| {
            assert_eq!(archived_locals(shell).len(), 2);
            assert!(shell.cloud_archive.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn a_cloud_session_terminal_launches_gh_codespace_ssh_from_home(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.workspace.active = Some(id);
            shell.new_terminal(window, cx);
            let terminal = shell.terminals[&shell.workspace.project]
                .last()
                .unwrap()
                .clone();
            let terminal = terminal.read(cx);
            let spec = terminal.launched().expect("launched, not a login shell");
            let args: Vec<_> = spec
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert_eq!(args, ["codespace", "ssh", "-c", "octo-abc"]);
            assert!(spec.program.to_string_lossy().ends_with("gh"));
            assert_eq!(terminal.cwd, crate::terminal::home_cwd());
        })
        .unwrap();
}

#[gpui::test]
fn the_codespace_menu_item_only_applies_to_local_github_projects(cx: &mut TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.offers_codespace(), "unchecked project");
            shell
                .github_projects
                .insert(shell.workspace.project.clone(), true);
            assert!(shell.offers_codespace());
            shell
                .github_projects
                .insert(shell.workspace.project.clone(), false);
            assert!(!shell.offers_codespace());
        })
        .unwrap();
}

fn cloud_row(shell: &mut Shell, codespace: &str) -> i64 {
    let id = add_cloud_session(&shell.store, "/work/monocode", codespace);
    let summary = locked(&shell.store)
        .unwrap()
        .sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id == id)
        .unwrap();
    shell.session_summaries.insert(
        id,
        Session {
            has_messages: true,
            ..summary
        },
    );
    shell.order.push(id);
    shell.cloud_marks.insert(
        id,
        CloudMark {
            local_project: "/work/monocode".into(),
            codespace: codespace.into(),
            kind: CloudBackendKind::Codespaces,
        },
    );
    id
}

#[gpui::test]
fn multi_select_archives_locals_now_and_asks_once_for_every_cloud_session(cx: &mut TestAppContext) {
    let (window, a, gh) = shell_with_cloud(cx);
    let (b, c, local) = window
        .update(cx, |shell, _, _| {
            let b = cloud_row(shell, "octo-abc");
            let c = cloud_row(shell, "octo-xyz");
            let local = locked(&shell.store)
                .unwrap()
                .create("claude", "/work/monocode")
                .unwrap();
            shell.session_summaries.insert(local.id, local.clone());
            shell.order.push(local.id);
            (b, c, local.id)
        })
        .unwrap();
    window
        .update(cx, |shell, _, cx| {
            for id in [a, b, c, local] {
                shell.toggle_archive(id, cx);
            }
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert!(
                shell.session_metadata(local, cx).summary.archived,
                "local archives at once"
            );
            let dialog = shell.cloud_archive.as_ref().expect("one dialog");
            let mut ids = dialog.ids();
            ids.sort();
            let mut want = vec![a, b, c];
            want.sort();
            assert_eq!(ids, want);
            let counts: Vec<_> = dialog
                .groups
                .iter()
                .map(|g| (g.codespace.clone(), g.ids.len()))
                .collect();
            assert_eq!(
                counts,
                [("octo-abc".to_string(), 2), ("octo-xyz".to_string(), 1)]
            );
            assert!(cloud::archive_summary("", &dialog.groups).contains("octo-abc (2 sessions)"));
        })
        .unwrap();
    window
        .update(visual, |shell, window, cx| {
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    let mut calls = gh.calls();
    calls.sort();
    assert_eq!(
        calls,
        ["codespace stop -c octo-abc", "codespace stop -c octo-xyz"]
    );
    window
        .update(visual, |shell, _, _| {
            for id in [a, b, c] {
                assert!(!shell.is_cloud(id));
            }
            assert_eq!(archived_locals(shell).len(), 3 + 1);
        })
        .unwrap();
}

#[gpui::test]
fn non_interactive_archives_never_show_the_dialog_or_touch_the_codespace(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| shell.toggle_archive_now(id, cx))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(gh.calls().is_empty());
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.cloud_archive.is_none());
            assert!(shell.cloud_archive_request.is_empty());
            assert!(shell.session_metadata(id, cx).summary.archived);
        })
        .unwrap();
}

#[gpui::test]
fn a_conversion_warning_is_a_notice_and_not_an_error(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            let inner = fake_convert(shell.store.clone(), None);
            shell.cloud_convert = Arc::new(move |svc, id, files, progress| {
                let mut done = inner(svc, id, files, progress)?;
                done.warnings =
                    vec!["1 ignored files or folders (about 1 MB) were not saved.".into()];
                Ok(done)
            });
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(!shell.is_cloud(id));
            assert_eq!(shell.error, None);
            assert_eq!(
                shell.notice.as_deref(),
                Some("1 ignored files or folders (about 1 MB) were not saved.")
            );
        })
        .unwrap();
    assert!(visual.debug_bounds("app-notice").is_some());
    assert!(visual.debug_bounds("app-error").is_none());
}

/// A conversion like `fake_convert` whose result `tweak` adjusts.
fn tweaked_convert(
    store: Database,
    tweak: impl Fn(&mut Converted) + Send + Sync + 'static,
) -> cloud::ConvertFn {
    let inner = fake_convert(store, None);
    Arc::new(move |svc, id, files, progress| {
        let mut done = inner(svc, id, files, progress)?;
        tweak(&mut done);
        Ok(done)
    })
}

fn with_env_file(done: &mut Converted) {
    done.ignored = vec![(638_976, "node_modules/".into()), (1, ".env".into())];
}

fn group(ids: Vec<i64>) -> cloud::CodespaceGroup {
    cloud::CodespaceGroup {
        codespace: "octo-abc".into(),
        kind: CloudBackendKind::Codespaces,
        ids,
        others: 0,
        total: 1,
    }
}

#[test]
fn unsaved_lines_name_real_files_skip_dependency_dirs_and_flag_partial_evidence() {
    let store = memory_store();
    let make = |tweak: &dyn Fn(&mut Converted)| {
        let id = add_cloud_session(&store, "/work/monocode", "octo-abc");
        let svc = CloudSessions::new(
            FakeGh::new(vec![]),
            Arc::new(FakeHost::default()),
            store.clone(),
        );
        let mut done = fake_convert(store.clone(), None)(&svc, id, false, &mut |_| {}).unwrap();
        tweak(&mut done);
        done
    };
    let clean = make(&|d| d.ignored = vec![(9, "web/node_modules/".into())]);
    assert!(cloud::unsaved_lines(&[clean]).is_empty());
    let many = make(&|d| {
        d.ignored = (0..5).map(|i| (1, format!("k{i}.key"))).collect();
        d.ignored_truncated = true;
    });
    let lines = cloud::unsaved_lines(&[many]);
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("k0.key, k1.key, k2.key and 2 more"),
        "{lines:?}"
    );
    assert!(lines[0].contains("even more"), "{lines:?}");
    let partial = make(&|d| d.evidence_partial = true);
    let lines = cloud::unsaved_lines(&[partial]);
    assert!(lines[0].contains("evidence") && lines[0].contains("could not be copied"));
}

#[test]
fn deleting_pauses_for_files_that_only_exist_on_the_codespace_but_stopping_never_does() {
    let run = |choice, tweak: fn(&mut Converted)| {
        let gh = FakeGh::new(vec![]);
        let store = memory_store();
        let id = add_cloud_session(&store, "/work/monocode", "octo-abc");
        let svc = CloudSessions::new(gh.clone(), Arc::new(FakeHost::default()), store.clone());
        let convert = tweaked_convert(store, tweak);
        let out = cloud::archive_groups(
            &svc,
            vec![group(vec![id])],
            choice,
            true,
            &convert,
            &AtomicBool::new(false),
            &[],
            &[],
            &mut |_| {},
        );
        (out, gh.archive_calls())
    };
    // A real ignored file: Delete pauses before touching the codespace.
    let (out, calls) = run(ArchiveChoice::Delete, with_env_file);
    assert_eq!(out[0].result.as_ref().unwrap_err(), cloud::NEEDS_REVIEW);
    assert_eq!(out[0].converted.len(), 1, "the copy is kept");
    assert!(out[0].risky[0].contains(".env") && !out[0].risky[0].contains("node_modules"));
    assert!(calls.is_empty(), "{calls:?}");
    // Partly copied evidence pauses too.
    let (out, calls) = run(ArchiveChoice::Delete, |d| d.evidence_partial = true);
    assert_eq!(out[0].result.as_ref().unwrap_err(), cloud::NEEDS_REVIEW);
    assert!(calls.is_empty());
    // Only dependency folders: nothing to ask, the codespace is deleted.
    let (out, calls) = run(ArchiveChoice::Delete, |d| {
        d.ignored = vec![(638_976, "node_modules/".into())]
    });
    assert!(out[0].result.is_ok());
    assert_eq!(calls, ["codespace delete -c octo-abc --force"]);
    // Stop keeps the codespace, so it never pauses (the notice comes afterwards).
    let (out, calls) = run(ArchiveChoice::Stop, with_env_file);
    assert!(out[0].result.is_ok());
    assert_eq!(calls, ["codespace stop -c octo-abc"]);
    let (out, _) = run(ArchiveChoice::Stop, |d| d.evidence_partial = true);
    assert!(out[0].result.is_ok());
}

#[gpui::test]
fn a_delete_with_unsaved_files_asks_then_delete_anyway_or_stop_instead(cx: &mut TestAppContext) {
    for stop_instead in [false, true] {
        let (window, id, gh) = shell_with_cloud(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.cloud_convert = tweaked_convert(shell.store.clone(), with_env_file);
                shell.open_cloud_archive(vec![id], window, cx);
                shell.confirm_cloud_archive(ArchiveChoice::Delete, window, cx);
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(
            gh.archive_calls().is_empty(),
            "paused before deleting: {:?}",
            gh.archive_calls()
        );
        window
            .update(visual, |shell, _, _| {
                let dialog = shell.cloud_archive.as_ref().expect("dialog stays open");
                match &dialog.phase {
                    cloud::Phase::Review { items } => assert!(items[0].contains(".env")),
                    _ => panic!("expected the review phase"),
                }
                // The local copy exists already.
                assert_eq!(archived_locals(shell).len(), 1);
            })
            .unwrap();
        for button in [
            "cloud-archive-review-keep",
            "cloud-archive-delete-anyway",
            "cloud-archive-stop-instead",
        ] {
            assert!(visual.debug_bounds(button).is_some(), "{button}");
        }
        window
            .update(visual, |shell, window, cx| {
                // What the two buttons do.
                let Some(cloud::Phase::Review { items }) =
                    shell.cloud_archive.as_ref().map(|d| &d.phase)
                else {
                    panic!("review phase");
                };
                let seen = items.clone();
                if stop_instead {
                    shell.run_cloud_archive(ArchiveChoice::Stop, false, Vec::new(), window, cx);
                } else {
                    shell.run_cloud_archive(ArchiveChoice::Delete, true, seen, window, cx);
                }
            })
            .unwrap();
        visual.run_until_parked();
        let expected = if stop_instead {
            "codespace stop -c octo-abc"
        } else {
            "codespace delete -c octo-abc --force"
        };
        assert_eq!(gh.archive_calls(), [expected]);
        window
            .update(visual, |shell, _, _| assert!(shell.cloud_archive.is_none()))
            .unwrap();
    }
}

/// Marks each session's ignored file by its cloud id, so two sessions lose different files.
fn env_per_session(done: &mut Converted) {
    done.ignored = vec![(1, format!("{}.env", done.cloud_id))];
}

#[gpui::test]
fn every_later_delete_asks_again_until_the_user_confirmed_the_list(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_convert = tweaked_convert(shell.store.clone(), env_per_session);
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Delete, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let review = |shell: &Shell| {
        matches!(
            shell.cloud_archive.as_ref().map(|d| &d.phase),
            Some(cloud::Phase::Review { .. })
        )
    };
    window
        .update(visual, |shell, _, _| assert!(review(shell)))
        .unwrap();
    // The Retry of the failed dialog and a fresh Delete are both a plain Delete with nothing
    // confirmed: the stored list stops them before any gh call.
    window
        .update(visual, |shell, window, cx| {
            shell.run_cloud_archive(ArchiveChoice::Delete, true, Vec::new(), window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(gh.archive_calls().is_empty(), "{:?}", gh.archive_calls());
    window
        .update(visual, |shell, _, _| assert!(review(shell)))
        .unwrap();
    // Keep running closes the dialog and leaves the codespace alone.
    window
        .update(visual, |shell, window, cx| {
            shell.close_cloud_archive(window, cx)
        })
        .unwrap();
    assert!(gh.archive_calls().is_empty());
    // The converted session offers stop and delete in its menu; delete asks again.
    let local = window
        .update(visual, |shell, _, _| archived_locals(shell)[0])
        .unwrap();
    window
        .update(visual, |shell, window, cx| {
            let ids: Vec<String> = shell
                .session_menu_items(local, cx)
                .into_iter()
                .filter_map(|item| match item {
                    crate::popover::MenuItem::Action(a) => Some(a.label.to_string()),
                    _ => None,
                })
                .collect();
            assert!(
                ids.contains(&"Stop codespace octo-abc".to_string()),
                "{ids:?}"
            );
            assert!(
                ids.contains(&"Delete codespace octo-abc".to_string()),
                "{ids:?}"
            );
            shell.run_origin_codespace_action(local, ArchiveChoice::Delete, window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(gh.archive_calls().is_empty(), "{:?}", gh.archive_calls());
    // The menu never deletes in one click: it confirms and names the stored files.
    window
        .update(visual, |shell, window, cx| {
            match shell.cloud_archive.as_ref().map(|d| &d.phase) {
                Some(cloud::Phase::ConfirmDelete { items }) => {
                    assert!(items[0].contains(".env"), "{items:?}")
                }
                _ => panic!("expected the delete confirmation"),
            }
            shell.run_cloud_archive(ArchiveChoice::Delete, true, Vec::new(), window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(
        gh.archive_calls().is_empty(),
        "stored list still gates: {:?}",
        gh.archive_calls()
    );
    window
        .update(visual, |shell, _, _| assert!(review(shell)))
        .unwrap();
    // Confirming deletes; the entries disappear with the codespace.
    window
        .update(visual, |shell, window, cx| {
            let Some(cloud::Phase::Review { items }) =
                shell.cloud_archive.as_ref().map(|d| &d.phase)
            else {
                panic!("review");
            };
            let seen = items.clone();
            shell.run_cloud_archive(ArchiveChoice::Delete, true, seen, window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(gh.archive_calls(), ["codespace delete -c octo-abc --force"]);
    window
        .update(visual, |shell, _, cx| {
            let labels = shell
                .session_menu_items(local, cx)
                .into_iter()
                .filter_map(|item| match item {
                    crate::popover::MenuItem::Action(a) => Some(a.id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert!(
                !labels
                    .iter()
                    .any(|l| l.starts_with("codespace-stop") || l.starts_with("codespace-delete"))
            );
        })
        .unwrap();
}

#[gpui::test]
fn delete_anyway_first_saves_a_session_that_appeared_while_the_dialog_was_open(
    cx: &mut TestAppContext,
) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_convert = tweaked_convert(shell.store.clone(), env_per_session);
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Delete, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    // A second session starts in the codespace while the dialog waits.
    let late = window
        .update(visual, |shell, _, _| {
            add_cloud_session(&shell.store, "/work/monocode", "octo-abc")
        })
        .unwrap();
    let confirm = |visual: &mut gpui::VisualTestContext| {
        window
            .update(visual, |shell, window, cx| {
                let Some(cloud::Phase::Review { items }) =
                    shell.cloud_archive.as_ref().map(|d| &d.phase)
                else {
                    panic!("review");
                };
                let seen = items.clone();
                shell.run_cloud_archive(ArchiveChoice::Delete, true, seen, window, cx);
            })
            .unwrap();
        visual.run_until_parked();
    };
    confirm(visual);
    // The late session was saved first, and its own unsaved file asks once more.
    window
        .update(visual, |shell, _, _| {
            assert!(!shell.cloud.is_cloud_session(late));
            assert_eq!(archived_locals(shell).len(), 2);
            assert!(matches!(
                shell.cloud_archive.as_ref().map(|d| &d.phase),
                Some(cloud::Phase::Review { .. })
            ));
        })
        .unwrap();
    assert!(gh.archive_calls().is_empty(), "{:?}", gh.archive_calls());
    confirm(visual);
    assert_eq!(gh.archive_calls(), ["codespace delete -c octo-abc --force"]);
}

#[gpui::test]
fn the_menu_delete_confirms_even_when_nothing_is_stored(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Keep, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    // Convert through Stop so a local copy with an origin exists.
    window
        .update(visual, |shell, window, cx| {
            shell.open_cloud_archive(vec![], window, cx);
            shell.cloud_origins.insert(id, "octo-abc".into());
            shell.cloud_marks.remove(&id);
            shell.run_origin_codespace_action(id, ArchiveChoice::Delete, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(gh.archive_calls().is_empty(), "no delete without a click");
    assert!(
        visual
            .debug_bounds("cloud-archive-confirm-delete")
            .is_some()
    );
    window
        .update(visual, |shell, window, cx| {
            shell.run_cloud_archive(ArchiveChoice::Delete, true, Vec::new(), window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(gh.archive_calls(), ["codespace delete -c octo-abc --force"]);
}

#[test]
fn a_failed_store_write_still_gates_the_delete_and_is_reported() {
    let gh = FakeGh::new(vec![]);
    let store = memory_store();
    let id = add_cloud_session(&store, "/work/monocode", "octo-abc");
    let svc = CloudSessions::new(gh.clone(), Arc::new(FakeHost::default()), store.clone());
    let poison = store.clone();
    // After converting, the settings store breaks (poisoned lock): nothing can be recorded.
    let convert = tweaked_convert(store, move |d| {
        with_env_file(d);
        // resume_unwind poisons the lock without running the panic hook.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = poison.lock().unwrap();
            std::panic::resume_unwind(Box::new("break the store"));
        }));
    });
    let out = cloud::archive_groups(
        &svc,
        vec![group(vec![id])],
        ArchiveChoice::Delete,
        true,
        &convert,
        &AtomicBool::new(false),
        &[],
        &[],
        &mut |_| {},
    );
    assert_eq!(out[0].result.as_ref().unwrap_err(), cloud::NEEDS_REVIEW);
    assert!(out[0].risky[0].contains(".env"), "{:?}", out[0].risky);
    assert!(gh.archive_calls().is_empty(), "{:?}", gh.archive_calls());
}

#[gpui::test]
fn a_partial_delete_still_drops_the_sessions_and_keeps_the_warning(cx: &mut TestAppContext) {
    struct FailingDisconnect(FakeHost);
    impl Host for FailingDisconnect {
        fn machines(&self) -> Result<Vec<Machine>, String> {
            self.0.machines()
        }
        fn connect(&self, c: &str, d: &str, p: &mut dyn FnMut(&str)) -> Result<Machine, String> {
            self.0.connect(c, d, p)
        }
        fn ensure(&self, m: &str, c: &AtomicBool, p: &mut dyn FnMut(&str)) -> Result<(), String> {
            self.0.ensure(m, c, p)
        }
        fn request(&self, m: &str, method: &str, params: Value) -> Result<Value, String> {
            self.0.request(m, method, params)
        }
        fn disconnect(&self, _: &str) -> Result<(), String> {
            Err("tunnel busy".into())
        }
    }
    let host = FailingDisconnect(FakeHost::default());
    host.0.machines.lock().unwrap().push(machine("octo-abc"));
    let svc = CloudSessions::new(FakeGh::new(vec![]), Arc::new(host), memory_store());
    let (then, warning) = cloud::apply_choice(
        &svc,
        ArchiveChoice::Delete,
        &CloudRef::codespace("octo-abc"),
    )
    .unwrap();
    assert_eq!(then, Then::Forget);
    assert!(
        warning
            .unwrap()
            .starts_with("Codespace deleted, but disconnecting failed")
    );
    let _ = cx;
}

#[gpui::test]
fn files_and_git_say_they_are_local_and_refuse_writes_while_a_cloud_session_is_active(
    cx: &mut TestAppContext,
) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.ensure_right_panel(window, cx);
            let panel = shell.right_panel.clone().expect("panel");
            shell.sync_cloud_panel(cx);
            assert_eq!(panel.read(cx).cloud_banner(), None);
            assert!(crate::git_gate::writes_allowed());
            shell.workspace.active = Some(id);
            shell.sync_cloud_panel(cx);
            assert_eq!(
                panel.read(cx).cloud_banner(),
                Some("Local files - this session runs in codespace octo-abc")
            );
            assert!(!crate::git_gate::writes_allowed());
            assert_eq!(crate::git_gate::codespace().as_deref(), Some("octo-abc"));
        })
        .unwrap();
}

#[test]
fn archive_copy_uses_the_session_title_clipped_with_a_fallback() {
    let group = |name: &str| cloud::CodespaceGroup {
        codespace: name.into(),
        kind: CloudBackendKind::Codespaces,
        ids: vec![1],
        others: 0,
        total: 1,
    };
    assert_eq!(
        cloud::archive_summary("Fix login", &[group("octo-abc")]),
        "“Fix login” runs in the codespace octo-abc."
    );
    assert_eq!(
        cloud::archive_summary("  ", &[group("octo-abc")]),
        "This session runs in the codespace octo-abc."
    );
    let long = cloud::quoted_title(&"word ".repeat(40));
    assert!(long.ends_with("…”") && long.chars().count() <= 52, "{long}");
}

#[test]
fn the_restore_message_names_the_branch_and_the_worktree_and_flags_conflicts() {
    let mut done = monocode_core::cloud_sessions::Restored {
        worktree: "/p-worktrees/feat-x".into(),
        branch: "feat/x".into(),
        head: "a".repeat(40),
        patch_applied: true,
        conflicts: None,
    };
    let text = cloud::restored_message(&done);
    assert!(text.contains("feat/x") && text.contains("/p-worktrees/feat-x"));
    assert!(!text.contains("did not apply"));
    done.conflicts = Some("error: patch failed: a.txt:1\nmore".into());
    let text = cloud::restored_message(&done);
    assert!(text.contains("did not apply") && text.contains("patch failed"));
    assert!(!text.contains("more"));
}

#[gpui::test]
fn the_restore_action_is_only_offered_for_converted_sessions(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| {
            // A cloud row and an in-memory store have nothing to restore.
            assert!(!shell.codespace_restore_pending(id));
            shell.restore_codespace_code(id, cx);
            assert!(shell.cloud_busy.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn delete_saves_every_session_of_the_codespace_but_stop_only_the_selected_ones(
    cx: &mut TestAppContext,
) {
    let (window, a, gh) = shell_with_cloud(cx);
    let b = window
        .update(cx, |shell, _, _| cloud_row(shell, "octo-abc"))
        .unwrap();
    // `b` is archived and not selected.
    window
        .update(cx, |shell, _, _| {
            shell.session_summaries.get_mut(&b).unwrap().archived = true;
        })
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::<i64>::new()));
    let log = calls.clone();
    window
        .update(cx, |shell, window, cx| {
            let inner = fake_convert(shell.store.clone(), None);
            shell.cloud_convert = Arc::new(move |cloud, id, files, progress| {
                log.lock().unwrap().push(id);
                inner(cloud, id, files, progress)
            });
            shell.open_cloud_archive(vec![a], window, cx);
            let dialog = shell.cloud_archive.as_ref().unwrap();
            assert!(cloud::delete_note(&dialog.groups).contains("all 2 sessions"));
            shell.confirm_cloud_archive(ArchiveChoice::Delete, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let mut seen = calls.lock().unwrap().clone();
    seen.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(seen, want);
    assert_eq!(gh.archive_calls(), ["codespace delete -c octo-abc --force"]);
    window
        .update(visual, |shell, _, _| {
            assert_eq!(archived_locals(shell).len(), 2)
        })
        .unwrap();
}

#[test]
fn a_cancelled_archive_stops_before_copying_or_touching_the_codespace() {
    let gh = FakeGh::new(vec![]);
    let svc = CloudSessions::new(gh.clone(), Arc::new(FakeHost::default()), memory_store());
    let convert = fake_convert(memory_store(), Some("must not run"));
    let cancel = AtomicBool::new(true);
    let group = cloud::CodespaceGroup {
        codespace: "octo-abc".into(),
        kind: CloudBackendKind::Codespaces,
        ids: vec![1],
        others: 0,
        total: 1,
    };
    let out = cloud::archive_groups(
        &svc,
        vec![group],
        ArchiveChoice::Delete,
        true,
        &convert,
        &cancel,
        &[],
        &[],
        &mut |_| {},
    );
    assert_eq!(out[0].result.as_ref().unwrap_err(), cloud::CANCELLED);
    assert!(out[0].converted.is_empty());
    assert!(gh.calls().is_empty());
}

#[gpui::test]
fn a_session_being_saved_takes_no_message_until_the_save_ends(cx: &mut TestAppContext) {
    let (window, a, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let view = shell.sessions[&a].clone();
            assert!(!view.update(cx, |v, cx| v.refuse_while_saving_copy(cx)));
            shell.lock_cloud_composer(&[a], true, cx);
            assert!(view.update(cx, |v, cx| v.refuse_while_saving_copy(cx)));
            shell.lock_cloud_composer(&[a], false, cx);
            assert!(!view.update(cx, |v, cx| v.refuse_while_saving_copy(cx)));
        })
        .unwrap();
}

/// A shell wired to a fake gh/host over its own store, and a begun start in a fresh checkout.
fn start_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::WindowHandle<Shell>,
    Arc<FakeGh>,
    Checkout,
    CloudSessions,
) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let gh = FakeGh::new(vec![
        ("auth status", AUTH_OK),
        ("codespace list", LIST),
        (
            "--method POST",
            r#"{"name":"octo-new1","state":"Provisioning"}"#,
        ),
        ("codespace view", r#"{"state":"Provisioning"}"#),
    ]);
    let checkout = github_checkout();
    let svc = window
        .update(cx, |shell, _, _| {
            let svc = CloudSessions::new(
                gh.clone(),
                Arc::new(FakeHost::default()),
                shell.store.clone(),
            );
            shell.cloud = Arc::new(CloudSessions::new(
                gh.clone(),
                Arc::new(FakeHost::default()),
                shell.store.clone(),
            ));
            svc
        })
        .unwrap();
    (window, gh, checkout, svc)
}

fn begin(
    svc: &CloudSessions,
    checkout: &Checkout,
) -> (Session, monocode_core::cloud_sessions::StartRecord) {
    let local = checkout.path().to_string_lossy().into_owned();
    let target = cloud_picker::target_for(CodespaceChoice::New, "feature", None).unwrap();
    cloud_picker::begin_cloud_start(svc, &local, target).unwrap()
}

#[gpui::test]
fn begin_start_opens_a_session_with_a_start_view_and_the_codespace_tab(cx: &mut TestAppContext) {
    use crate::{cloud_start_view::StartPhase, right_panel::RightTab};
    let (window, _, checkout, svc) = start_fixture(cx);
    let (summary, record) = begin(&svc, &checkout);
    let id = summary.id;
    window
        .update(cx, |shell, window, cx| {
            shell.open_cloud_start(summary, record, window, cx);
            assert_eq!(shell.workspace.active, Some(id));
            assert!(shell.sessions.contains_key(&id));
            let snapshot = shell.cloud_start_snapshot(id).unwrap();
            assert_eq!(snapshot.phase, StartPhase::Starting);
            assert!(snapshot.running);
            assert_eq!(snapshot.record.codespace, "octo-new1");
            // The conversation does not load the host while it starts.
            assert!(shell.sessions[&id].read(cx).cloud_start_blocks());
            let panel = shell.right_panel.clone().unwrap();
            assert_eq!(panel.read(cx).tab(), RightTab::Codespace);
            let pane = panel.read(cx).codespace.clone();
            assert!(pane.read(cx).has_start());
            assert_eq!(panel.read(cx).codespace_title(), "Codespace");
            assert!(shell.workspace.right_panel_open);
            assert!(shell.right_panel_shown(cx));
            shell.workspace.classic_layout = true;
            assert!(shell.right_panel_shown(cx));
            assert!(shell.cloud_marks.contains_key(&id));
            assert_ne!(
                shell.cloud_states.get("octo-new1"),
                Some(&CloudState::Available)
            );
        })
        .unwrap();
}

#[gpui::test]
fn ready_rebinds_the_conversation_and_queues_a_message_sent_while_starting(
    cx: &mut TestAppContext,
) {
    use crate::cloud_start_view::StartPhase;
    let (window, _, checkout, svc) = start_fixture(cx);
    let (summary, record) = begin(&svc, &checkout);
    let id = summary.id;
    window
        .update(cx, |shell, _, cx| {
            // A message typed while the codespace started.
            monocode_core::cloud_sessions::queue_start_message(
                &locked(&shell.store).unwrap(),
                id,
                "fix the bug",
            )
            .unwrap();
            shell
                .cloud_starts
                .insert(id, cloud_start::CloudStartView::new(record));
            shell.add_loaded(summary.clone(), cx);
            let chat = shell.sessions[&id].clone();
            assert!(chat.read(cx).summary.cwd.starts_with("remote://pending-"));
            let ready = Session {
                cwd: "remote://env-octo-new1/workspaces/widgets".into(),
                ..summary
            };
            // finish_ready needs a window only to restore the panel, which was not opened.
            let _ = &ready;
        })
        .unwrap();
    window
        .update(cx, |shell, window, cx| {
            let cwd = "remote://env-octo-new1/workspaces/widgets".to_string();
            let mut session = shell.session_summaries[&id].clone();
            session.cwd = cwd.clone();
            // The user is elsewhere (or the app restarted): no view exists for the session.
            shell.sessions.remove(&id);
            shell.finish_ready(id, session, window, cx);
            // A queued message needs a conversation to send it, so one is made.
            assert!(shell.sessions.contains_key(&id));
            let chat = shell.sessions[&id].read(cx);
            assert_eq!(chat.summary.cwd, cwd);
            assert!(!chat.cloud_start_blocks());
            assert_eq!(chat.cloud_start().unwrap().phase, StartPhase::Ready);
            assert!(chat.cloud_start().unwrap().ready.is_some());
            assert_eq!(shell.session_summaries[&id].cwd, cwd);
            assert!(shell.cloud_start_label(id).is_none());
        })
        .unwrap();
}

#[gpui::test]
fn restart_resumes_a_running_start_and_only_shows_a_failed_one(cx: &mut TestAppContext) {
    use crate::cloud_start_view::StartPhase;
    use monocode_core::cloud_sessions::startup::save_start_record;
    let (window, gh, checkout, svc) = start_fixture(cx);
    let (running, running_record) = begin(&svc, &checkout);
    let (failed, mut failed_record) = begin(&svc, &checkout);
    failed_record.error = Some("Container build failed".into());
    window
        .update(cx, |shell, _, cx| {
            save_start_record(&locked(&shell.store).unwrap(), &failed_record).unwrap();
            // At startup the shell already holds every stored session summary.
            for session in locked(&shell.store).unwrap().sessions().unwrap() {
                shell.session_summaries.insert(session.id, session);
            }
            let summaries = locked(&shell.store).unwrap().sessions().unwrap();
            let store = shell.store.clone();
            let fresh = cx.new(|cx| {
                Shell::new(
                    store,
                    summaries,
                    checkout.path().to_string_lossy().into_owned(),
                    None,
                    cx,
                )
            });
            assert!(fresh.read(cx).session_summaries[&running.id].has_messages);
            assert!(fresh.read(cx).session_summaries[&failed.id].has_messages);
            assert!(fresh.read(cx).cloud_start_snapshot(failed.id).is_some());
            let before = gh.calls().len();
            shell.resume_cloud_starts(cx);
            let resumed = shell.cloud_start_snapshot(running.id).unwrap();
            assert_eq!(resumed.phase, StartPhase::Starting);
            assert!(resumed.running, "a start that did not fail resumes");
            let shown = shell.cloud_start_snapshot(failed.id).unwrap();
            assert_eq!(shown.phase, StartPhase::Failed);
            assert!(!shown.running, "a failed start waits for Retry");
            assert_eq!(shown.error(), Some("Container build failed"));
            assert_eq!(running_record.session_id, running.id);
            // Both sessions are listed under the local project and carry the right dot.
            assert!(shell.cloud_marks.contains_key(&running.id));
            assert!(shell.session_summaries[&failed.id].has_messages);
            assert_eq!(
                shell.cloud_start_label(running.id).unwrap().label,
                "Starting"
            );
            assert_eq!(
                shell.cloud_start_label(running.id).unwrap().tone,
                Tone::Busy
            );
            assert_eq!(shell.cloud_start_label(failed.id).unwrap().label, "Failed");
            assert_eq!(shell.cloud_start_label(failed.id).unwrap().tone, Tone::Bad);
            assert!(shell.cloud_badge(failed.id, false).is_some());
            // The failed start ran nothing against gh.
            assert_eq!(gh.calls().len(), before);
            // A second call never starts a second worker.
            shell.track_cloud_start(running.id, cx);
        })
        .unwrap();
}

#[gpui::test]
fn the_codespace_tab_is_never_the_saved_right_tab(cx: &mut TestAppContext) {
    use crate::right_panel::RightTab;
    let (window, _, checkout, svc) = start_fixture(cx);
    let (summary, record) = begin(&svc, &checkout);
    let id = summary.id;
    window
        .update(cx, |shell, window, cx| {
            let saved = shell.workspace.right_panel_tab;
            shell
                .cloud_starts
                .insert(id, cloud_start::CloudStartView::new(record));
            shell.add_loaded(summary, cx);
            shell.workspace.active = Some(id);
            shell.show_codespace_panel(id, window, cx);
            let panel = shell.right_panel.clone().unwrap();
            assert_eq!(panel.read(cx).tab(), RightTab::Codespace);
            assert_eq!(shell.workspace.right_panel_tab, saved);
            assert_ne!(shell.workspace.right_panel_tab, RightTab::Codespace);
            assert!(!panel.read(cx).tabs().contains(&RightTab::Codespace));
            let state = serde_json::to_string(&shell.workspace).unwrap();
            assert!(!state.contains("codespace"), "{state}");
            assert_eq!(
                RightTab::normalize(&[RightTab::Codespace, RightTab::Graph]),
                [RightTab::Graph, RightTab::Files]
            );
        })
        .unwrap();
}

/// Inventory-only boundary: any SSH, lifecycle, or backup command fails the test.
struct InventoryGh {
    response: Result<GhOutput, String>,
    calls: Mutex<Vec<String>>,
}
impl InventoryGh {
    fn missing() -> Arc<Self> {
        Arc::new(Self {
            response: Ok(GhOutput {
                success: true,
                stdout: r#"[{"total_count":0,"codespaces":[]}]"#.into(),
                stderr: String::new(),
            }),
            calls: Mutex::new(vec![]),
        })
    }
}
impl GhRunner for InventoryGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        let call = args.join(" ");
        self.calls.lock().unwrap().push(call.clone());
        match call.as_str() {
            "auth status --active" => Ok(GhOutput {
                success: true,
                stdout: AUTH_OK.into(),
                stderr: String::new(),
            }),
            "api --paginate --slurp user/codespaces" => self.response.clone(),
            _ => panic!("unexpected remote operation: {call}"),
        }
    }
}

#[gpui::test]
fn deleting_a_stale_cloud_session_succeeds_without_contacting_its_host(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    let gh = InventoryGh::missing();
    let host = Arc::new(FakeHost::default());
    window
        .update(cx, |shell, w, cx| {
            shell.cloud = Arc::new(CloudSessions::new(
                gh.clone(),
                host.clone(),
                shell.store.clone(),
            ));
            shell.cloud_convert = fake_convert(shell.store.clone(), Some("backup must not run"));
            {
                let store = locked(&shell.store).unwrap();
                let mut remote = store.remote_session(id).unwrap().unwrap();
                remote.session_id = Some("host-session".into());
                store.save_remote_session(&remote).unwrap();
            }
            shell.request_delete_sessions(vec![id], w, cx);
            shell.confirm_delete_dialog(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(
                !shell.session_summaries.contains_key(&id),
                "{:?}",
                shell.error
            );
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .remote_session(id)
                    .unwrap()
                    .is_none()
            );
            assert!(shell.error.is_none(), "{:?}", shell.error);
        })
        .unwrap();
    assert!(host.log.lock().unwrap().is_empty());
    assert_eq!(gh.calls.lock().unwrap().len(), 2);
}

#[gpui::test]
fn deleting_an_already_missing_codespace_skips_backup_and_forgets_its_sessions(
    cx: &mut TestAppContext,
) {
    let (window, id, _) = shell_with_cloud(cx);
    let gh = InventoryGh::missing();
    let host = Arc::new(FakeHost::default());
    window
        .update(cx, |shell, w, cx| {
            shell.cloud = Arc::new(CloudSessions::new(
                gh.clone(),
                host.clone(),
                shell.store.clone(),
            ));
            shell.cloud_convert =
                Arc::new(|_, _, _, _| panic!("must not back up a missing codespace"));
            shell.open_cloud_archive(vec![id], w, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Delete, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.session_summaries.contains_key(&id));
            assert!(shell.cloud_archive.is_none());
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .remote_session(id)
                    .unwrap()
                    .is_none()
            );
        })
        .unwrap();
    assert!(host.log.lock().unwrap().is_empty());
}

#[gpui::test]
fn failed_inventory_keeps_shell_sessions_and_never_attempts_backup_or_host_deletion(
    cx: &mut TestAppContext,
) {
    for response in [
        Err("network timeout".into()),
        Ok(GhOutput {
            success: false,
            stdout: String::new(),
            stderr: "HTTP 403: Forbidden".into(),
        }),
        Ok(GhOutput {
            success: false,
            stdout: String::new(),
            stderr: "HTTP 404: Not Found".into(),
        }),
    ] {
        for archive in [false, true] {
            let (window, id, _) = shell_with_cloud(cx);
            let gh = Arc::new(InventoryGh {
                response: response.clone(),
                calls: Mutex::new(vec![]),
            });
            let host = Arc::new(FakeHost::default());
            window
                .update(cx, |shell, w, cx| {
                    shell.cloud = Arc::new(CloudSessions::new(
                        gh.clone(),
                        host.clone(),
                        shell.store.clone(),
                    ));
                    shell.cloud_convert =
                        Arc::new(|_, _, _, _| panic!("lookup failure must not attempt backup"));
                    {
                        let store = locked(&shell.store).unwrap();
                        let mut remote = store.remote_session(id).unwrap().unwrap();
                        remote.session_id = Some("host-session".into());
                        store.save_remote_session(&remote).unwrap();
                    }
                    if archive {
                        shell.open_cloud_archive(vec![id], w, cx);
                        shell.confirm_cloud_archive(ArchiveChoice::Delete, w, cx);
                    } else {
                        shell.request_delete_sessions(vec![id], w, cx);
                        shell.confirm_delete_dialog(w, cx);
                    }
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(cx, |shell, _, _| {
                    assert!(shell.session_summaries.contains_key(&id));
                    assert!(
                        locked(&shell.store)
                            .unwrap()
                            .remote_session(id)
                            .unwrap()
                            .is_some()
                    );
                    if archive {
                        assert!(matches!(
                            shell.cloud_archive.as_ref().unwrap().phase,
                            cloud::Phase::Failed { .. }
                        ));
                    } else {
                        assert!(
                            shell
                                .error
                                .as_ref()
                                .unwrap()
                                .starts_with("Could not delete the session:")
                        );
                    }
                })
                .unwrap();
            assert!(host.log.lock().unwrap().is_empty());
        }
    }
}

// ---------------------------------------------------------------- Docker sandbox sessions

const SBX_NAME: &str = "claude-widgets-ab12";
const SBX_ID: &str = "sbx_1abc";

/// Answers `sbx ls` / `inspect` / `stop` / `rm` for a list of `(id, name, state)` and records
/// every call.
struct FakeSbx {
    sandboxes: Mutex<Vec<(String, String, String)>>,
    calls: Mutex<Vec<String>>,
}
impl FakeSbx {
    fn new(state: &str) -> Arc<Self> {
        Arc::new(Self {
            sandboxes: Mutex::new(vec![(SBX_ID.into(), SBX_NAME.into(), state.into())]),
            calls: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// The verbs that changed something.
    fn changes(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.contains(" stop ") || c.contains(" rm "))
            .collect()
    }
}
impl monocode_core::sandboxes::SbxRunner for FakeSbx {
    fn run(
        &self,
        args: &[String],
        _: Option<&str>,
    ) -> Result<monocode_core::sandboxes::SbxOutput, String> {
        self.calls.lock().unwrap().push(args.join(" "));
        let verb = args.get(2).map(String::as_str).unwrap_or("");
        let items: Vec<Value> = self
            .sandboxes
            .lock()
            .unwrap()
            .iter()
            .map(|(id, name, state)| json!({"id":id,"name":name,"agent":"shell","state":state}))
            .collect();
        let stdout = match verb {
            "ls" => serde_json::to_string(&items).unwrap(),
            "inspect" => items[0].to_string(),
            _ => String::new(),
        };
        Ok(monocode_core::sandboxes::SbxOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

/// An ssh seam that fails the test when anything runs through it: opening, listing, stopping
/// and removing must never ssh into (and so resume) a sandbox.
struct NoSsh;
impl monocode_core::cloud_sessions::backend::docker::SandboxShell for NoSsh {
    fn run(
        &self,
        _: &str,
        remote: &str,
        _: &str,
        _: &mut dyn FnMut(&str),
        _: &dyn Fn() -> bool,
        _: std::time::Duration,
    ) -> Result<monocode_core::cloud_sessions::backend::docker::ShellOutput, String> {
        panic!("ssh must not run in this test: {remote}");
    }
}

fn sandbox_machine() -> Machine {
    Machine {
        id: "m-sbx".into(),
        name: SBX_NAME.into(),
        endpoint: format!("sbx://{SBX_NAME}"),
        environment_id: format!("env-{SBX_NAME}"),
        ssh: None,
        codespace: None,
        sandbox: Some(monocode_core::remote::SandboxTarget {
            id: SBX_ID.into(),
            name: SBX_NAME.into(),
            workspace: "/home/agent/widgets".into(),
            relay_port: 47200,
            config: None,
            mode: Default::default(),
        }),
    }
}

fn sandbox_service(sbx: &Arc<FakeSbx>, with_machine: bool, store: Database) -> CloudSessions {
    let host = Arc::new(FakeHost::default());
    if with_machine {
        host.machines.lock().unwrap().push(sandbox_machine());
    }
    CloudSessions::new(FakeGh::new(vec![]), host, store.clone()).with_sandboxes(
        sbx.clone(),
        Arc::new(NoSsh),
        Arc::new(monocode_core::cloud_sessions::docker_start::StoreSecrets::new(store)),
    )
}

/// Stores a sandbox cloud session of `local` and returns its id.
fn add_sandbox_session(store: &Database, local: &str) -> i64 {
    let store = store.lock().unwrap();
    let project = store
        .remember_cloud_ref_project(
            &format!("env-{SBX_NAME}"),
            &json!({"id":"p","cwd":"/home/agent/widgets"}),
            local,
            &CloudRef::sandbox(SBX_NAME),
        )
        .unwrap();
    let mut summary = store.create("claude", &project.key).unwrap();
    summary.has_messages = true;
    summary.id
}

/// Like `shell_with_cloud`, with a sandbox session whose sandbox is in `state`.
fn shell_with_sandbox(
    cx: &mut TestAppContext,
    state: &str,
    display: CloudState,
    with_machine: bool,
) -> (gpui::WindowHandle<Shell>, i64, Arc<FakeSbx>) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let sbx = FakeSbx::new(state);
    let id = window
        .update(cx, |shell, _, _| {
            let id = add_sandbox_session(&shell.store, "/work/monocode");
            let summary = locked(&shell.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap();
            shell.cloud = Arc::new(sandbox_service(&sbx, with_machine, shell.store.clone()));
            shell.cloud_convert = fake_convert(shell.store.clone(), None);
            shell.session_summaries.insert(
                id,
                Session {
                    has_messages: true,
                    ..summary
                },
            );
            shell.order.push(id);
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: SBX_NAME.into(),
                    kind: CloudBackendKind::DockerSandbox,
                },
            );
            shell
                .cloud_states
                .insert(CloudRef::sandbox(SBX_NAME).key(), display);
            id
        })
        .unwrap();
    (window, id, sbx)
}

fn menu_labels(shell: &Shell, id: i64, cx: &App) -> Vec<String> {
    shell
        .session_menu_items(id, cx)
        .into_iter()
        .filter_map(|item| match item {
            crate::popover::MenuItem::Action(a) => Some(a.label.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn collect_lists_sandbox_sessions_next_to_codespaces_and_never_uses_ssh() {
    let store = memory_store();
    let local = github_checkout();
    let local_path = local.path().to_string_lossy().into_owned();
    let sandbox_id = add_sandbox_session(&store, &local_path);
    let codespace_id = add_cloud_session(&store, &local_path, "octo-abc");
    let sbx = FakeSbx::new("running");
    let gh = FakeGh::new(vec![("codespace view", r#"{"state":"Shutdown"}"#)]);
    let host = Arc::new(FakeHost::default());
    let svc = CloudSessions::new(gh, host, store.clone()).with_sandboxes(
        sbx.clone(),
        Arc::new(NoSsh),
        Arc::new(monocode_core::cloud_sessions::docker_start::StoreSecrets::new(store)),
    );
    let snapshot = cloud::collect(&svc, std::slice::from_ref(&local_path), &[]);
    let mark = snapshot.marks.get(&sandbox_id).expect("sandbox is listed");
    assert_eq!(mark.kind, CloudBackendKind::DockerSandbox);
    assert_eq!(mark.codespace, SBX_NAME);
    assert_eq!(
        snapshot.marks[&codespace_id].kind,
        CloudBackendKind::Codespaces
    );
    // The state is cached by the backend-aware key: bare name for a codespace.
    assert_eq!(
        snapshot.states.get(&format!("sbx:{SBX_NAME}")),
        Some(&CloudState::Available)
    );
    assert_eq!(snapshot.states.get("octo-abc"), Some(&CloudState::Stopped));
    assert!(snapshot.activity.contains_key(&sandbox_id));
    // Only read-only sbx verbs ran (and NoSsh would have panicked).
    assert!(sbx.changes().is_empty(), "{:?}", sbx.calls());
}

#[gpui::test]
fn a_sandbox_row_has_a_badge_and_its_state_comes_from_the_sandbox_key(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_sandbox(cx, "stopped", CloudState::Stopped, true);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.cloud_badge(id, false).is_some());
            let mark = shell.cloud_marks[&id].clone();
            assert_eq!(mark.key(), format!("sbx:{SBX_NAME}"));
            assert_eq!(
                shell.cloud_states.get(&mark.key()),
                Some(&CloudState::Stopped)
            );
            shell.workspace.active = Some(id);
            assert_eq!(shell.active_cloud(), Some(CloudRef::sandbox(SBX_NAME)));
        })
        .unwrap();
}

#[test]
fn the_archive_copy_names_the_sandbox_and_keeps_the_codespace_text_byte_identical() {
    let group = |kind, name: &str, ids: Vec<i64>, total| cloud::CodespaceGroup {
        codespace: name.into(),
        kind,
        ids,
        others: 0,
        total,
    };
    let sandbox = group(CloudBackendKind::DockerSandbox, SBX_NAME, vec![1], 1);
    let codespace = group(CloudBackendKind::Codespaces, "octo-abc", vec![1], 1);
    // Single session.
    assert_eq!(
        cloud::archive_summary("Fix login", std::slice::from_ref(&sandbox)),
        format!("“Fix login” runs in the sandbox {SBX_NAME}.")
    );
    assert_eq!(
        cloud::archive_summary("Fix login", std::slice::from_ref(&codespace)),
        "“Fix login” runs in the codespace octo-abc."
    );
    // Cost sentence.
    assert_eq!(
        cloud::words(std::slice::from_ref(&sandbox)).cost(),
        "A running sandbox keeps costing money until its runtime limit stops it."
    );
    assert_eq!(
        cloud::words(std::slice::from_ref(&codespace)).cost(),
        "A running codespace keeps costing money until it idles out."
    );
    // Delete note, one and several.
    assert_eq!(
        cloud::delete_note(std::slice::from_ref(&sandbox)),
        "Delete sandbox saves a local copy of all 1 session in the sandbox."
    );
    assert_eq!(
        cloud::delete_note(std::slice::from_ref(&codespace)),
        "Delete codespace saves a local copy of all 1 session in the codespace."
    );
    let two = [
        group(CloudBackendKind::DockerSandbox, "a-box", vec![1], 1),
        group(CloudBackendKind::DockerSandbox, "b-box", vec![2, 3], 2),
    ];
    assert_eq!(
        cloud::delete_note(&two),
        "Delete sandbox saves a local copy of all 3 sessions in the sandboxes."
    );
    assert_eq!(
        cloud::archive_summary("x", &two),
        "3 cloud sessions run in: a-box (1 session), b-box (2 sessions). The choice applies to each sandbox."
    );
    // A mixed selection uses neutral words.
    let mixed = [sandbox.clone(), codespace.clone()];
    assert_eq!(cloud::words(&mixed).noun, "workspace");
    assert_eq!(
        cloud::delete_note(&mixed),
        "Delete workspace saves a local copy of all 2 sessions in the workspaces."
    );
    // Other users, review, failure and confirm text.
    let (sb, cs) = (cloud::words(&[sandbox]), cloud::words(&[codespace]));
    assert_eq!(
        cloud::others_note(2, sb.noun).as_deref(),
        Some("2 other sessions use this sandbox.")
    );
    assert_eq!(
        cloud::delete_confirm_line(&sb, SBX_NAME),
        format!("Delete the sandbox {SBX_NAME}? This cannot be undone.")
    );
    assert_eq!(
        cloud::delete_confirm_line(&cs, "octo-abc"),
        "Delete the codespace octo-abc? This cannot be undone."
    );
    assert_eq!(
        cloud::review_note(&sb),
        "The local copy is saved without them. Deleting the sandbox removes them for good; \
         stopping it keeps them. Keep running leaves it as it is: stop or delete it later \
         from the session menu."
    );
    assert_eq!(
        cloud::review_note(&cs),
        "The local copy is saved without them. Deleting the codespace removes them for good; \
         stopping it keeps them. Keep running leaves it as it is: stop or delete it later \
         from the session menu."
    );
    assert_eq!(
        cloud::failed_note(&sb),
        "Going on without a complete local copy can lose the code and files that only exist on the sandbox."
    );
    assert_eq!(
        cloud::failed_note(&cs),
        "Going on without a complete local copy can lose the code and files that only exist on the codespace."
    );
    // Stop is the default when nothing else uses the workspace.
    assert_eq!(cloud::default_choice(0), ArchiveChoice::Stop);
    assert_eq!(
        cloud::banner_text(&CloudRef::sandbox("box")),
        "Local files - this session runs in sandbox box"
    );
    assert_eq!(
        cloud::banner_text(&CloudRef::codespace("octo-abc")),
        "Local files - this session runs in codespace octo-abc"
    );
}

#[gpui::test]
fn archiving_a_sandbox_session_offers_sandbox_buttons_and_stop_calls_sbx_stop(
    cx: &mut TestAppContext,
) {
    let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, _, cx| shell.toggle_archive(id, cx))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let dialog = shell.cloud_archive.as_ref().expect("dialog opened");
            assert_eq!(dialog.ids(), vec![id]);
            assert_eq!(dialog.groups[0].kind, CloudBackendKind::DockerSandbox);
            assert_eq!(cloud::words(&dialog.groups).noun, "sandbox");
        })
        .unwrap();
    for button in [
        "cloud-archive-cancel",
        "cloud-archive-keep",
        "cloud-archive-delete",
        "cloud-archive-stop",
    ] {
        assert!(visual.debug_bounds(button).is_some(), "{button}");
    }
    window
        .update(visual, |shell, window, cx| {
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    // `sbx stop <id>` only: no ssh (NoSsh would panic), no rm.
    assert_eq!(
        sbx.changes()
            .iter()
            .map(|c| c.rsplit(' ').next().unwrap().to_string())
            .collect::<Vec<_>>(),
        [SBX_ID]
    );
    assert!(sbx.changes()[0].contains(" stop "), "{:?}", sbx.calls());
    window
        .update(visual, |shell, _, _| {
            assert!(shell.cloud_archive.is_none());
            assert!(!shell.is_cloud(id));
            assert_eq!(archived_locals(shell).len(), 1);
            // Origins are codespace-only: the archived copy offers no codespace actions.
            let local = archived_locals(shell)[0];
            assert!(shell.live_origin_codespace(local).is_none());
        })
        .unwrap();
}

#[gpui::test]
fn deleting_from_a_sandbox_menu_confirms_with_sandbox_wording_then_removes_it(
    cx: &mut TestAppContext,
) {
    let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, window, cx| {
            shell.run_session_menu(id, "sandbox-delete", window, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let dialog = shell.cloud_archive.as_ref().expect("confirm dialog");
            assert!(matches!(dialog.phase, cloud::Phase::ConfirmDelete { .. }));
            assert_eq!(dialog.groups[0].cloud(), CloudRef::sandbox(SBX_NAME));
        })
        .unwrap();
    assert!(
        visual
            .debug_bounds("cloud-archive-confirm-delete")
            .is_some()
    );
    assert!(sbx.changes().is_empty(), "nothing before the confirmation");
    window
        .update(visual, |shell, window, cx| {
            shell.run_cloud_archive(ArchiveChoice::Delete, true, Vec::new(), window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(
        sbx.changes()
            .iter()
            .any(|c| c.contains(" rm ") && c.ends_with(SBX_ID)),
        "{:?}",
        sbx.calls()
    );
    window
        .update(visual, |shell, _, _| {
            assert!(!shell.is_cloud(id));
            assert!(shell.cloud_archive.is_none());
            assert!(!shell.cloud_states.contains_key(&format!("sbx:{SBX_NAME}")));
        })
        .unwrap();
}

#[gpui::test]
fn the_sandbox_menu_stops_and_deletes_by_name_and_removes_a_stopped_one_locally(
    cx: &mut TestAppContext,
) {
    // Running: terminal, stop and delete; nothing to remove locally.
    let (window, id, _) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, _, cx| {
            let labels = menu_labels(shell, id, cx);
            for expected in [
                "Open terminal in sandbox".to_string(),
                format!("Stop sandbox {SBX_NAME}"),
                format!("Delete sandbox {SBX_NAME}"),
            ] {
                assert!(labels.contains(&expected), "{expected}: {labels:?}");
            }
            assert!(!labels.contains(&"Remove from Monoptah".to_string()));
            assert!(
                !labels.iter().any(|l| l.contains("codespace")),
                "{labels:?}"
            );
        })
        .unwrap();
    // Stopped: no Stop, but Remove from Monoptah.
    let (window, id, sbx) = shell_with_sandbox(cx, "stopped", CloudState::Stopped, true);
    window
        .update(cx, |shell, _, cx| {
            let labels = menu_labels(shell, id, cx);
            assert!(
                !labels.contains(&format!("Stop sandbox {SBX_NAME}")),
                "{labels:?}"
            );
            assert!(labels.contains(&format!("Delete sandbox {SBX_NAME}")));
            assert!(
                labels.contains(&"Remove from Monoptah".to_string()),
                "{labels:?}"
            );
        })
        .unwrap();
    window
        .update(cx, |shell, window, cx| {
            shell.run_session_menu(id, "sandbox-remove-local", window, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    // A confirmation comes first; nothing is removed yet.
    window
        .update(visual, |shell, _, _| {
            assert!(shell.is_cloud(id));
            assert!(matches!(
                shell.cloud_archive.as_ref().unwrap().phase,
                cloud::Phase::ConfirmRemove { .. }
            ));
        })
        .unwrap();
    assert!(visual.debug_bounds("cloud-archive-cancel").is_some());
    let go = visual
        .debug_bounds("cloud-archive-remove-go")
        .expect("Remove");
    visual.simulate_click(go.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.cloud_archive.is_none());
            assert!(!shell.is_cloud(id));
            assert!(!shell.session_summaries.contains_key(&id));
            let notice = shell.notice.clone().expect("the store's warning");
            assert!(notice.contains("still exists"), "{notice}");
            assert!(notice.contains("keeps costing money"), "{notice}");
            assert!(!notice.contains("session menu"), "{notice}");
            assert!(
                notice.contains(&format!("sbx --cloud rm {SBX_NAME}")),
                "{notice}"
            );
        })
        .unwrap();
    // The sandbox itself was not touched (and NoSsh never ran).
    assert!(sbx.changes().is_empty(), "{:?}", sbx.calls());
}

#[gpui::test]
fn a_codespace_menu_keeps_its_entries_and_has_no_sandbox_ones(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| {
            let labels = menu_labels(shell, id, cx);
            assert!(
                labels.contains(&"Open terminal in codespace".to_string()),
                "{labels:?}"
            );
            assert!(
                !labels
                    .iter()
                    .any(|l| l.contains("sandbox") || l == "Remove from Monoptah"),
                "{labels:?}"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_sandbox_terminal_runs_ssh_into_the_container_and_a_missing_connection_says_so(
    cx: &mut TestAppContext,
) {
    let (window, id, _) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, window, cx| {
            shell.workspace.active = Some(id);
            shell.new_terminal(window, cx);
            let terminal = shell.terminals[&shell.workspace.project]
                .last()
                .unwrap()
                .clone();
            let terminal = terminal.read(cx);
            let spec = terminal.launched().expect("launched, not a login shell");
            let args: Vec<String> = spec
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert!(spec.program.to_string_lossy().ends_with("ssh"));
            assert_eq!(args[0], "-t");
            let n = args.len();
            assert_eq!(args[n - 3], "--");
            assert_eq!(args[n - 2], format!("{SBX_ID}@sbx_cloud"));
            let remote = &args[n - 1];
            assert!(
                remote.contains("devcontainer exec --workspace-folder"),
                "{remote}"
            );
            assert!(remote.contains("/home/agent/widgets"), "{remote}");
            assert!(remote.ends_with("-- bash -l"), "{remote}");
            assert!(spec.env_set.iter().any(|(key, _)| key == "PATH"));
            assert_eq!(terminal.cwd, crate::terminal::home_cwd());
        })
        .unwrap();
    // Without a stored connection: the reason on screen, no dock tab, nothing spawned.
    let (window, id, _) = shell_with_sandbox(cx, "running", CloudState::Available, false);
    window
        .update(cx, |shell, window, cx| {
            shell.workspace.active = Some(id);
            let tabs = |shell: &Shell| {
                shell
                    .workspace
                    .docks
                    .get(&shell.workspace.project)
                    .map_or(0, |d| d.terminals.len())
            };
            let (tabs_before, spawned_before) = (
                tabs(shell),
                shell
                    .terminals
                    .get(&shell.workspace.project)
                    .map_or(0, Vec::len),
            );
            shell.new_terminal(window, cx);
            assert_eq!(tabs(shell), tabs_before);
            assert_eq!(
                shell
                    .terminals
                    .get(&shell.workspace.project)
                    .map_or(0, Vec::len),
                spawned_before
            );
            let error = shell.error.clone().expect("the reason");
            assert!(
                error.contains("Could not open a terminal in the sandbox"),
                "{error}"
            );
            assert!(error.contains("connection was removed"), "{error}");
        })
        .unwrap();
}

#[gpui::test]
fn background_terminal_paths_never_ssh_into_a_sandbox(cx: &mut TestAppContext) {
    // Render, a tab switch or closing the last terminal call `ensure_panel_terminal`; with a
    // sandbox session in view it must not launch an ssh (NoSsh and the spec would show it).
    let (window, id, sbx) = shell_with_sandbox(cx, "stopped", CloudState::Stopped, true);
    window
        .update(cx, |shell, _, cx| {
            shell.workspace.active = Some(id);
            shell.ensure_panel_terminal(cx);
            // Another project with an empty dock, the same session still in view.
            shell.workspace.project = "/work/other".into();
            shell.ensure_panel_terminal(cx);
            for terminals in shell.terminals.values() {
                for terminal in terminals {
                    let terminal = terminal.read(cx);
                    assert!(
                        terminal.launched().is_none(),
                        "a sandbox must not get a launched ssh terminal"
                    );
                }
            }
        })
        .unwrap();
    assert!(sbx.changes().is_empty(), "{:?}", sbx.calls());
}

#[test]
fn mixed_selections_use_neutral_words_and_same_names_of_different_kinds_stay_apart() {
    let mark = |kind| CloudMark {
        local_project: "/work/monocode".into(),
        codespace: "same-name".into(),
        kind,
    };
    let marks: HashMap<i64, CloudMark> = [
        (1, mark(CloudBackendKind::Codespaces)),
        (2, mark(CloudBackendKind::DockerSandbox)),
        (3, mark(CloudBackendKind::DockerSandbox)),
    ]
    .into_iter()
    .collect();
    let groups = cloud::group_by_codespace(&[1, 2, 3], &marks, |target, _| {
        usize::from(target.kind == CloudBackendKind::Codespaces)
    });
    assert_eq!(groups.len(), 2, "different kinds never merge");
    let sandbox = groups
        .iter()
        .find(|g| g.kind == CloudBackendKind::DockerSandbox)
        .unwrap();
    assert_eq!(
        (sandbox.ids.clone(), sandbox.total, sandbox.others),
        (vec![2, 3], 2, 0)
    );
    let codespace = groups
        .iter()
        .find(|g| g.kind == CloudBackendKind::Codespaces)
        .unwrap();
    assert_eq!(
        (codespace.ids.clone(), codespace.total, codespace.others),
        (vec![1], 1, 1)
    );
    let words = cloud::words(&groups);
    assert_eq!(
        (words.noun, words.title, words.plural),
        ("workspace", "Workspace", "workspaces")
    );
    assert!(
        words
            .cost()
            .starts_with("A running workspace keeps costing money")
    );
}

#[test]
fn the_sandbox_terminal_args_carry_the_connection_options_and_the_delete_note_is_sandbox_only() {
    let args = cloud::sandbox_ssh_args(&monocode_core::cloud_sessions::SandboxTerminal {
        destination: "sbx_1abc@sbx_cloud".into(),
        remote: "devcontainer exec -- bash -l".into(),
    });
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    for option in ["ConnectTimeout=120", "ServerAliveInterval=15"] {
        assert!(args.contains(&option.to_string()), "{option}: {args:?}");
    }
    let sb = cloud::words(&[cloud::CodespaceGroup {
        codespace: "a".into(),
        kind: CloudBackendKind::DockerSandbox,
        ids: vec![],
        others: 0,
        total: 1,
    }]);
    assert_eq!(
        cloud::delete_start_note(&sb, true).as_deref(),
        Some("Deleting saves a local copy first, which starts the sandbox for a moment.")
    );
    assert_eq!(cloud::delete_start_note(&sb, false), None);
    let cs = cloud::words(&[cloud::CodespaceGroup {
        codespace: "a".into(),
        kind: CloudBackendKind::Codespaces,
        ids: vec![],
        others: 0,
        total: 1,
    }]);
    assert_eq!(cloud::delete_start_note(&cs, true), None);
    assert_eq!(
        cloud::remove_confirm_body("Fix  login", "box-1"),
        "Remove \u{2018}Fix login\u{2019} from Monoptah? The sandbox box-1 keeps existing and keeps \
         costing money until you delete it (`sbx --cloud rm box-1`). Its conversation is not saved \
         on this computer."
    );
}

#[gpui::test]
fn a_failed_group_never_drags_an_unreachable_one_into_delete_anyway(cx: &mut TestAppContext) {
    let (window, sandbox_session, sbx) =
        shell_with_sandbox(cx, "running", CloudState::Available, true);
    let codespace_session = window
        .update(cx, |shell, _, _| {
            let id = add_cloud_session(&shell.store, "/work/monocode", "octo-abc");
            let summary = locked(&shell.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap();
            shell.session_summaries.insert(
                id,
                Session {
                    has_messages: true,
                    ..summary
                },
            );
            shell.order.push(id);
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            id
        })
        .unwrap();
    window
        .update(cx, |shell, window, cx| {
            let inner = fake_convert(shell.store.clone(), None);
            shell.cloud_convert = Arc::new(move |svc, id, agreed, progress| {
                if id == codespace_session {
                    Err(ConvertError::Failed("disk full".into()))
                } else if !agreed {
                    Err(ConvertError::NeedsFilesOnlyConfirm {
                        reason: "the Host did not answer".into(),
                    })
                } else {
                    inner(svc, id, agreed, progress)
                }
            });
            shell.open_cloud_archive(vec![codespace_session, sandbox_session], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let dialog = shell.cloud_archive.as_ref().unwrap();
            assert!(matches!(dialog.phase, cloud::Phase::Failed { .. }));
            // "Stop anyway" would reach only the failed group.
            assert_eq!(dialog.groups.len(), 1);
            assert_eq!(dialog.groups[0].kind, CloudBackendKind::Codespaces);
        })
        .unwrap();
    window
        .update(visual, |shell, window, cx| {
            shell.run_cloud_archive(ArchiveChoice::Stop, false, Vec::new(), window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(
        sbx.changes().is_empty(),
        "the unreachable sandbox was not touched"
    );
    window
        .update(visual, |shell, _, _| {
            let dialog = shell.cloud_archive.as_ref().expect("files-only step");
            match &dialog.phase {
                cloud::Phase::FilesOnly { id, reason, .. } => {
                    assert_eq!(*id, sandbox_session);
                    assert_eq!(reason, "the Host did not answer");
                }
                _ => panic!("expected the files-only step"),
            }
            assert_eq!(dialog.groups.len(), 1);
            assert!(shell.is_cloud(sandbox_session));
        })
        .unwrap();
}

#[gpui::test]
fn a_dead_sandbox_session_can_be_archived_from_its_banner_action(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, window, cx| {
            // The session has no start view: the banner action still reaches the archive flow.
            assert!(!shell.cloud_starts.contains_key(&id));
            shell.cloud_start_action(
                id,
                crate::cloud_start_view::StartAction::ArchiveSession,
                window,
                cx,
            );
            let dialog = shell.cloud_archive.as_ref().expect("archive dialog");
            assert_eq!(dialog.ids(), vec![id]);
        })
        .unwrap();
}

#[gpui::test]
fn a_sandbox_start_opens_the_sandbox_tab_without_a_browser_action(cx: &mut TestAppContext) {
    use crate::{cloud_start_view::StartAction, right_panel::RightTab};
    let (window, _, _) = crate::shell::tests::window(cx);
    let gh = FakeGh::new(vec![]);
    let (id, record) = window
        .update(cx, |shell, _, _| {
            shell.cloud = Arc::new(CloudSessions::new(
                gh.clone(),
                Arc::new(FakeHost::default()),
                shell.store.clone(),
            ));
            let id = add_sandbox_session(&shell.store, "/work/monocode");
            let mut record = crate::cloud_start_view::fixtures::record(2);
            record.session_id = id;
            record.local_project = "/work/monocode".into();
            record.codespace = String::new();
            record.sandbox = Some(SBX_NAME.into());
            (id, record)
        })
        .unwrap();
    window
        .update(cx, |shell, window, cx| {
            let summary = locked(&shell.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap();
            shell.open_cloud_start(summary, record, window, cx);
            let mark = &shell.cloud_marks[&id];
            assert_eq!(mark.kind, CloudBackendKind::DockerSandbox);
            assert_eq!(mark.codespace, SBX_NAME);
            let key = format!("sbx:{SBX_NAME}");
            assert!(shell.cloud_states.contains_key(&key));
            assert_ne!(shell.cloud_states.get(&key), Some(&CloudState::Available));
            let snapshot = shell.cloud_start_snapshot(id).unwrap();
            assert_eq!(
                snapshot.headline(),
                "Starting sandbox \u{b7} Building container"
            );
            let panel = shell.right_panel.clone().unwrap();
            assert_eq!(panel.read(cx).tab(), RightTab::Codespace);
            assert_eq!(panel.read(cx).codespace_title(), "Sandbox");
            // A stray Open in browser reaches neither gh nor the notification line.
            shell.cloud_start_action(id, StartAction::OpenInBrowser, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        gh.calls().iter().all(|c| !c.contains("codespace")),
        "{:?}",
        gh.calls()
    );
    window
        .update(cx, |shell, _, cx| {
            // Switching to a session with no start view names the tab as before.
            shell.workspace.active = None;
            shell.sync_codespace_pane(cx);
            let panel = shell.right_panel.clone().unwrap();
            assert_eq!(panel.read(cx).codespace_title(), "Codespace");
        })
        .unwrap();
}

// ------------------------------------------------ files-only archive and rebuild text

/// A convert that cannot reach the conversation until the user agreed to save only the code
/// (`needs_confirm`), or never could (a recreated container: no question, history lost).
fn history_lost_convert(store: Database, needs_confirm: bool) -> cloud::ConvertFn {
    let inner = fake_convert(store, None);
    Arc::new(move |svc, id, agreed, progress| {
        if needs_confirm && !agreed {
            return Err(ConvertError::NeedsFilesOnlyConfirm {
                reason: "the Host did not answer".into(),
            });
        }
        let mut done = inner(svc, id, agreed, progress)?;
        done.history_saved = false;
        Ok(done)
    })
}

#[test]
fn the_files_only_text_quotes_the_title_and_the_rebuild_text_follows_the_backend() {
    assert_eq!(
        cloud::files_only_body("Fix  login", "the Host did not answer"),
        "The conversation of \u{2018}Fix login\u{2019} could not be reached (the Host did not \
         answer). You can retry, or save only the code (unpushed commits and uncommitted \
         changes) and continue. The conversation will not be saved."
    );
    assert!(
        cloud::files_only_body("  ", "x").starts_with("The conversation of this session could")
    );
    let (title, body) = cloud_start::rebuild_prompt(CloudBackendKind::DockerSandbox);
    assert_eq!(title, "Rebuild this sandbox?");
    assert_eq!(
        body,
        "This interrupts every session using this sandbox and recreates its container. The \
         conversation history of those sessions is lost; archive a session first to keep it. \
         Code in the workspace is kept."
    );
    let (title, body) = cloud_start::rebuild_prompt(CloudBackendKind::Codespaces);
    assert_eq!(title, "Rebuild this codespace?");
    assert_eq!(
        body,
        "This interrupts every session using this codespace and recreates its container."
    );
}

#[gpui::test]
fn an_unreachable_conversation_asks_before_archiving_files_only_then_continues(
    cx: &mut TestAppContext,
) {
    let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_convert = history_lost_convert(shell.store.clone(), true);
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let dialog = shell.cloud_archive.as_ref().expect("dialog stays open");
            match &dialog.phase {
                cloud::Phase::FilesOnly { reason, .. } => {
                    assert_eq!(reason, "the Host did not answer")
                }
                _ => panic!("expected the files-only step"),
            }
            assert!(shell.is_cloud(id), "nothing was archived yet");
        })
        .unwrap();
    for button in [
        "cloud-archive-cancel",
        "cloud-archive-files-retry",
        "cloud-archive-files-only-go",
    ] {
        assert!(visual.debug_bounds(button).is_some(), "{button}");
    }
    assert!(sbx.changes().is_empty(), "{:?}", sbx.calls());
    // Retry asks again (the convert still cannot reach it).
    window
        .update(visual, |shell, window, cx| {
            shell.run_cloud_archive(ArchiveChoice::Stop, true, Vec::new(), window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(matches!(
                shell.cloud_archive.as_ref().unwrap().phase,
                cloud::Phase::FilesOnly { .. }
            ))
        })
        .unwrap();
    window
        .update(visual, |shell, window, cx| {
            shell.confirm_files_only(ArchiveChoice::Stop, id, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(sbx.changes().len(), 1, "{:?}", sbx.calls());
    window
        .update(visual, |shell, _, _| {
            assert!(shell.cloud_archive.is_none());
            assert!(!shell.is_cloud(id));
            let notice = shell.notice.clone().expect("history note");
            assert!(notice.contains("could not be saved"), "{notice}");
        })
        .unwrap();
}

#[gpui::test]
fn deleting_after_files_only_goes_through_the_not_saved_review(cx: &mut TestAppContext) {
    let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_convert = history_lost_convert(shell.store.clone(), true);
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(ArchiveChoice::Delete, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, window, cx| {
            shell.confirm_files_only(ArchiveChoice::Delete, id, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            match &shell.cloud_archive.as_ref().expect("review").phase {
                cloud::Phase::Review { items } => assert!(
                    items.iter().any(|l| l.contains("was not saved")),
                    "{items:?}"
                ),
                _ => panic!("expected the review step"),
            }
        })
        .unwrap();
    assert!(
        sbx.changes().is_empty(),
        "nothing deleted before the review"
    );
}

#[gpui::test]
fn a_recreated_container_archives_files_only_without_asking_and_says_so(cx: &mut TestAppContext) {
    let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, window, cx| {
            // Core archives a recreated container files-only without a question.
            shell.cloud_convert = history_lost_convert(shell.store.clone(), false);
            shell.cloud_start_action(
                id,
                crate::cloud_start_view::StartAction::ArchiveSession,
                window,
                cx,
            );
            assert!(shell.cloud_archive.is_some());
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.cloud_archive.is_none(), "no question was asked");
            assert!(!shell.is_cloud(id));
            let notice = shell.notice.clone().expect("history note");
            assert!(
                notice.contains("The conversation of") && notice.contains("could not be saved"),
                "{notice}"
            );
        })
        .unwrap();
    assert_eq!(sbx.changes().len(), 1);
}

// ---- "RUN ON CLOUD" from the new-session composer ----

/// A draft session of `local` listed in the shell, as the new-session composer shows it, with
/// the project known to be a GitHub repository.
fn add_draft(
    shell: &mut Shell,
    local: &str,
    cx: &mut Context<Shell>,
) -> (i64, Entity<crate::conversation::Conversation>) {
    let summary = locked(&shell.store)
        .unwrap()
        .create("claude", local)
        .unwrap();
    let id = summary.id;
    shell.github_projects.insert(local.to_owned(), true);
    shell.add_loaded(summary, cx);
    shell.sync_codespace_offers(cx);
    (id, shell.sessions[&id].clone())
}

#[gpui::test]
fn create_and_open_a_codespace_creates_it_queues_the_prompt_and_opens_the_start(
    cx: &mut TestAppContext,
) {
    use crate::{cloud_start_view::StartPhase, conversation::cloud_target::CloudTarget};
    let (window, gh, checkout, _svc) = start_fixture(cx);
    let local = checkout.path().to_string_lossy().into_owned();
    for args in [
        vec!["symbolic-ref", "HEAD", "refs/heads/feature"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&local)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let (draft, chat) = window
        .update(cx, |shell, _, cx| add_draft(shell, &local, cx))
        .unwrap();
    window
        .update(cx, |_, _, cx| {
            chat.update(cx, |chat, cx| {
                assert!(chat.cloud_menu_choice("workspace:cloud-codespace", cx));
                assert_eq!(chat.cloud_target(), Some(CloudTarget::Codespace));
                chat.input
                    .update(cx, |input, cx| input.set_text("fix the bug".into(), cx));
                // Pending: sending (Enter or the button) creates; this test drives the same path.
                assert!(chat.cloud_create_pending());
                chat.create_cloud(cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let created = gh.calls();
            assert!(
                created.iter().any(|call| call.contains("--method POST")),
                "{created:?}"
            );
            let (id, snapshot) = shell
                .cloud_starts
                .keys()
                .filter_map(|id| Some((id, shell.cloud_start_snapshot(*id)?)))
                .find(|(_, snapshot)| snapshot.record.codespace == "octo-new1")
                .expect("a start was opened for the new codespace");
            assert_ne!(*id, draft);
            assert_eq!(snapshot.record.branch.as_deref(), Some("feature"));
            assert_eq!(shell.workspace.active, Some(*id));
            assert_eq!(snapshot.phase, StartPhase::Starting);
            // The composer text is the first prompt of the new session, queued once.
            assert_eq!(
                monocode_core::cloud_sessions::queued_start_messages(
                    &locked(&shell.store).unwrap(),
                    *id
                ),
                vec!["fix the bug".to_owned()]
            );
            let _ = cx;
            // The draft composer is spent: no target, no text.
            let chat = chat.read(cx);
            assert_eq!(chat.cloud_target(), None);
            assert_eq!(chat.input.read(cx).content.as_ref(), "");
        })
        .unwrap();
}

#[gpui::test]
fn create_and_open_docker_passes_the_config_and_branch_and_shows_a_refusal_inline(
    cx: &mut TestAppContext,
) {
    use crate::conversation::cloud_target::{CloudCreate, CloudTarget};
    use monocode_core::cloud_sessions::docker_config::{self, DockerProjectConfig};
    let (window, _, _) = crate::shell::tests::window(cx);
    let seen: Arc<std::sync::Mutex<Vec<CloudCreate>>> = Arc::default();
    let sink = seen.clone();
    let (draft, chat) = window
        .update(cx, |shell, _, cx| {
            shell.cloud_create_hook = Some(Arc::new(move |request| {
                sink.lock().unwrap().push(request.clone());
                Err("Docker sandboxes are not available.".into())
            }));
            let out = add_draft(shell, "/work/widgets", cx);
            let config = DockerProjectConfig {
                repo_url: "https://github.com/octo/widgets.git".into(),
                ..Default::default()
            };
            docker_config::save(&locked(&shell.store).unwrap(), "/work/widgets", &config).unwrap();
            out
        })
        .unwrap();
    let sessions = window
        .update(cx, |shell, _, cx| {
            chat.update(cx, |chat, cx| {
                chat.cloud_menu_choice("workspace:cloud-docker", cx);
                chat.input
                    .update(cx, |input, cx| input.set_text("run tests".into(), cx));
                chat.create_cloud(cx);
            });
            shell.sessions.len()
        })
        .unwrap();
    cx.run_until_parked();
    let sent = seen.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].target, CloudTarget::DockerSandbox);
    assert_eq!(sent[0].project, "/work/widgets");
    assert!(sent[0].branch.starts_with("monoptah/"));
    assert_eq!(sent[0].prompt.as_deref(), Some("run tests"));
    assert_eq!(
        sent[0].config.as_ref().unwrap().repo_url,
        "https://github.com/octo/widgets.git"
    );
    window
        .update(cx, |shell, _, cx| {
            // Nothing was created: no new session, the draft keeps its text and the error.
            assert_eq!(shell.sessions.len(), sessions);
            assert!(shell.sessions.contains_key(&draft));
            assert!(shell.cloud_starts.is_empty());
            let chat = chat.read(cx);
            assert_eq!(chat.input.read(cx).content.as_ref(), "run tests");
            assert_eq!(chat.cloud_target(), Some(CloudTarget::DockerSandbox));
            assert!(chat.cloud_create_pending());
        })
        .unwrap();
}

#[gpui::test]
fn a_sandbox_start_is_marked_by_its_key_and_a_failed_one_is_not_tracked(cx: &mut TestAppContext) {
    use crate::{cloud_start_view::StartPhase, conversation::cloud_target::CloudCreate};
    let (window, _, checkout, svc) = start_fixture(cx);
    let local = checkout.path().to_string_lossy().into_owned();
    let (summary, mut record) = begin(&svc, &checkout);
    record.codespace = String::new();
    record.sandbox = Some("demo-1".into());
    let mut failed = record.clone();
    failed.error = Some("create timed out".into());
    let ok = (summary.clone(), record.clone());
    let bad = (summary.clone(), failed);
    let fail_next = Arc::new(AtomicBool::new(false));
    let flag = fail_next.clone();
    let seen: Arc<std::sync::Mutex<Vec<CloudCreate>>> = Arc::default();
    let sink = seen.clone();
    let (_, chat) = window
        .update(cx, |shell, _, cx| {
            shell.cloud_create_hook = Some(Arc::new(move |request| {
                sink.lock().unwrap().push(request.clone());
                Ok(if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    bad.clone()
                } else {
                    ok.clone()
                })
            }));
            add_draft(shell, &local, cx)
        })
        .unwrap();
    let create = |cx: &mut TestAppContext| {
        window
            .update(cx, |_, _, cx| {
                chat.update(cx, |chat, cx| {
                    chat.cloud_menu_choice("workspace:cloud-codespace", cx);
                    chat.input
                        .update(cx, |input, cx| input.set_text("go".into(), cx));
                    chat.create_cloud(cx);
                })
            })
            .unwrap();
        cx.run_until_parked();
    };
    create(cx);
    window
        .update(cx, |shell, _, _| {
            let id = summary.id;
            let snapshot = shell.cloud_start_snapshot(id).unwrap();
            assert_eq!(snapshot.phase, StartPhase::Starting);
            assert!(snapshot.running);
            // Queued by the worker, before the session opened.
            assert_eq!(
                monocode_core::cloud_sessions::queued_start_messages(
                    &locked(&shell.store).unwrap(),
                    id
                ),
                vec!["go".to_owned()]
            );
            // Start over.
            shell.drop_cloud_start(id);
            shell.cloud_marks.remove(&id);
        })
        .unwrap();
    // Right after opening (before a refresh reads the store) the mark carries the sandbox key.
    window
        .update(cx, |shell, window, cx| {
            shell.open_cloud_start(summary.clone(), record, window, cx);
            let mark = &shell.cloud_marks[&summary.id];
            assert_eq!(mark.codespace, "demo-1");
            assert_eq!(
                mark.kind,
                monocode_core::cloud_sessions::CloudBackendKind::DockerSandbox
            );
            assert!(shell.cloud_states.contains_key("sbx:demo-1"));
            shell.drop_cloud_start(summary.id);
            shell.cloud_marks.remove(&summary.id);
        })
        .unwrap();
    fail_next.store(true, std::sync::atomic::Ordering::Relaxed);
    create(cx);
    window
        .update(cx, |shell, _, _| {
            let snapshot = shell.cloud_start_snapshot(summary.id).unwrap();
            assert_eq!(snapshot.phase, StartPhase::Failed);
            assert!(!snapshot.running, "a failed start waits for Retry");
        })
        .unwrap();
}

#[gpui::test]
fn a_live_codespace_menu_stops_without_converting_and_hides_the_entry_when_stopped(
    cx: &mut TestAppContext,
) {
    let (window, id, gh) = shell_with_cloud(cx);
    // Stopped: no entry.
    window
        .update(cx, |shell, _, cx| {
            let labels = menu_labels(shell, id, cx);
            assert!(
                !labels.iter().any(|l| l.starts_with("Stop codespace")),
                "{labels:?}"
            );
            shell
                .cloud_states
                .insert("octo-abc".into(), CloudState::Available);
            let labels = menu_labels(shell, id, cx);
            assert!(
                labels.contains(&"Stop codespace octo-abc".to_string()),
                "{labels:?}"
            );
            shell
                .cloud_states
                .insert("octo-abc".into(), CloudState::Stopping);
            let labels = menu_labels(shell, id, cx);
            assert!(
                !labels.iter().any(|l| l.starts_with("Stop codespace")),
                "{labels:?}"
            );
            shell
                .cloud_states
                .insert("octo-abc".into(), CloudState::Available);
        })
        .unwrap();
    window
        .update(cx, |shell, window, cx| {
            shell.run_session_menu(id, "codespace-stop-live", window, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert_eq!(gh.calls(), ["codespace stop -c octo-abc"]);
    window
        .update(visual, |shell, _, _| {
            // No conversion: the session is still the cloud session and nothing is archived.
            assert!(shell.is_cloud(id));
            assert!(shell.cloud_archive.is_none());
            assert!(archived_locals(shell).is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn a_shared_codespace_says_the_stop_also_stops_the_other_sessions(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| {
            shell
                .cloud_states
                .insert("octo-abc".into(), CloudState::Available);
            let other = id + 1000;
            shell.cloud_marks.insert(
                other,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            let labels = menu_labels(shell, id, cx);
            assert!(
                labels
                    .contains(&"Stop codespace octo-abc (also stops 1 other session)".to_string()),
                "{labels:?}"
            );
        })
        .unwrap();
}

#[gpui::test]
fn the_live_stop_entry_needs_a_named_codespace_in_a_running_state_and_counts_only_live_peers(
    cx: &mut TestAppContext,
) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| {
            let has_stop = |shell: &Shell, cx: &App| {
                menu_labels(shell, id, cx)
                    .iter()
                    .any(|l| l.starts_with("Stop codespace"))
            };
            for (state, shown) in [
                (CloudState::Available, true),
                (CloudState::Starting, true),
                (CloudState::Unknown, true),
                (CloudState::Stopped, false),
                (CloudState::Stopping, false),
                (CloudState::Unavailable, false),
            ] {
                shell.cloud_states.insert("octo-abc".into(), state);
                assert_eq!(has_stop(shell, cx), shown, "{state:?}");
            }
            // An archived peer and a nameless start do not count as sessions to stop.
            shell
                .cloud_states
                .insert("octo-abc".into(), CloudState::Available);
            let archived = id + 1000;
            let nameless = id + 1001;
            for other in [archived, nameless] {
                shell.cloud_marks.insert(
                    other,
                    CloudMark {
                        local_project: "/work/monocode".into(),
                        codespace: if other == nameless {
                            String::new()
                        } else {
                            "octo-abc".into()
                        },
                        kind: CloudBackendKind::Codespaces,
                    },
                );
            }
            let mut summary = shell.session_summaries[&id].clone();
            summary.id = archived;
            summary.archived = true;
            shell.session_summaries.insert(archived, summary);
            let labels = menu_labels(shell, id, cx);
            assert!(
                labels.contains(&"Stop codespace octo-abc".to_string()),
                "{labels:?}"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_codespace_start_without_a_name_has_no_stop_entry(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: String::new(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            shell
                .cloud_states
                .insert(String::new(), CloudState::Unknown);
            let labels = menu_labels(shell, id, cx);
            assert!(
                !labels.iter().any(|l| l.starts_with("Stop codespace")),
                "{labels:?}"
            );
        })
        .unwrap();
}

#[gpui::test]
fn stopping_a_codespace_with_a_running_turn_asks_first(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, window, cx| {
            shell
                .cloud_states
                .insert("octo-abc".into(), CloudState::Available);
            // A turn runs in another session of the same codespace.
            let other = id + 1000;
            shell.cloud_marks.insert(
                other,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            shell.remote_marks.insert(other, (true, false));
            shell.run_session_menu(id, "codespace-stop-live", window, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
    window
        .update(visual, |shell, _, _| {
            assert!(matches!(
                shell.cloud_archive.as_ref().map(|d| &d.phase),
                Some(cloud::Phase::ConfirmStop)
            ));
        })
        .unwrap();
    assert!(visual.debug_bounds("cloud-archive-cancel").is_some());
    let go = visual
        .debug_bounds("cloud-archive-confirm-stop")
        .expect("Stop");
    visual.simulate_click(go.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(gh.calls(), ["codespace stop -c octo-abc"]);
    window
        .update(visual, |shell, _, _| {
            assert!(shell.cloud_archive.is_none());
            assert!(shell.is_cloud(id));
        })
        .unwrap();
}

#[gpui::test]
fn stopping_a_sandbox_with_a_running_turn_asks_first_and_cancel_stops_nothing(
    cx: &mut TestAppContext,
) {
    let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, window, cx| {
            shell.remote_marks.insert(id, (true, false));
            shell.run_session_menu(id, "sandbox-stop", window, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(matches!(
                shell.cloud_archive.as_ref().map(|d| &d.phase),
                Some(cloud::Phase::ConfirmStop)
            ));
        })
        .unwrap();
    assert!(visual.debug_bounds("cloud-archive-confirm-stop").is_some());
    window
        .update(visual, |shell, window, cx| {
            shell.close_cloud_archive(window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(sbx.changes().is_empty(), "{:?}", sbx.calls());
    // Without a running turn the stop is direct, as before.
    window
        .update(visual, |shell, window, cx| {
            shell.remote_marks.insert(id, (false, false));
            shell.run_session_menu(id, "sandbox-stop", window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(sbx.changes()[0].contains(" stop "), "{:?}", sbx.calls());
}

// ------------------------------------------------------------ resume entry of the menu

fn resume_label(shell: &Shell, id: i64, cx: &App) -> Option<String> {
    menu_labels(shell, id, cx)
        .into_iter()
        .find(|label| label.starts_with("Resume "))
}

#[gpui::test]
fn the_session_menu_offers_resume_only_for_a_stopped_sandbox(cx: &mut TestAppContext) {
    for (state, offered) in [
        (CloudState::Stopped, true),
        (CloudState::Available, false),
        (CloudState::Starting, false),
        (CloudState::Stopping, false),
        (CloudState::Unavailable, false),
        (CloudState::Unknown, false),
    ] {
        let (window, id, _) = shell_with_sandbox(cx, "stopped", state, true);
        window
            .update(cx, |shell, _, cx| {
                let expected = format!("Resume sandbox {SBX_NAME}");
                assert_eq!(
                    resume_label(shell, id, cx),
                    offered.then_some(expected),
                    "{state:?}"
                );
            })
            .unwrap();
    }
    // No name yet: nothing to resume.
    let (window, id, _) = shell_with_sandbox(cx, "stopped", CloudState::Stopped, true);
    window
        .update(cx, |shell, _, cx| {
            shell.cloud_marks.get_mut(&id).unwrap().codespace = String::new();
            assert_eq!(resume_label(shell, id, cx), None);
        })
        .unwrap();
}

#[gpui::test]
fn the_session_menu_offers_resume_only_for_a_stopped_codespace(cx: &mut TestAppContext) {
    for (state, offered) in [
        (CloudState::Stopped, true),
        (CloudState::Available, false),
        (CloudState::Starting, false),
        (CloudState::Stopping, false),
        (CloudState::Unavailable, false),
        (CloudState::Unknown, false),
    ] {
        let (window, id, _) = shell_with_cloud(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.cloud_states.insert("octo-abc".into(), state);
                assert_eq!(
                    resume_label(shell, id, cx),
                    offered.then(|| "Resume codespace octo-abc".to_string()),
                    "{state:?}"
                );
            })
            .unwrap();
    }
    let (window, id, _) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.cloud_marks.get_mut(&id).unwrap().codespace = String::new();
            assert_eq!(resume_label(shell, id, cx), None);
        })
        .unwrap();
}

#[gpui::test]
fn a_menu_resume_for_a_workspace_that_is_not_stopped_does_nothing(cx: &mut TestAppContext) {
    let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, window, cx| {
            shell.run_session_menu(id, "cloud-resume", window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.sessions.contains_key(&id), "no view was opened");
        })
        .unwrap();
    assert!(sbx.changes().is_empty(), "{:?}", sbx.calls());
}

#[gpui::test]
fn the_shell_tells_each_conversation_whether_its_workspace_is_stopped(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_sandbox(cx, "stopped", CloudState::Stopped, true);
    window
        .update(cx, |shell, _, cx| {
            let view = shell.ensure_session(id, cx).unwrap();
            shell.sync_cloud_stopped(cx);
            assert!(view.read(cx).cloud_stopped_for_test());
            shell
                .cloud_states
                .insert(CloudRef::sandbox(SBX_NAME).key(), CloudState::Available);
            shell.sync_cloud_stopped(cx);
            assert!(!view.read(cx).cloud_stopped_for_test());
        })
        .unwrap();
}

#[test]
fn the_create_request_turns_the_panel_picks_into_begin_start_arguments() {
    use crate::conversation::cloud_target::{CloudCreate, CloudTarget};
    let request = |machine: Option<&str>| CloudCreate {
        target: CloudTarget::Codespace,
        project: "/work/widgets".into(),
        branch: "release/1".into(),
        config: None,
        machine: machine.map(str::to_owned),
        prompt: None,
        request_id: 1,
    };
    let picked =
        super::cloud_create::codespace_target(&request(Some("standardLinux32gb"))).unwrap();
    assert_eq!(picked.branch.as_deref(), Some("release/1"));
    assert_eq!(picked.machine.as_deref(), Some("standardLinux32gb"));
    // No pick keeps the default machine, chosen later by the core.
    let default = super::cloud_create::codespace_target(&request(None)).unwrap();
    assert_eq!(default.machine, None);
}

#[gpui::test]
fn a_resume_during_a_refresh_drops_that_stale_answer_and_reads_again(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_sandbox(cx, "stopped", CloudState::Stopped, true);
    let key = CloudRef::sandbox(SBX_NAME).key();
    window
        .update(cx, |shell, _, cx| {
            let stale = cloud::Snapshot {
                marks: shell.cloud_marks.clone(),
                states: [(key.clone(), CloudState::Stopped)].into_iter().collect(),
                ..Default::default()
            };
            // A refresh is in flight when the user resumes.
            shell.cloud_refreshing = true;
            shell.cloud_resumed(id, cx);
            assert_eq!(shell.cloud_states.get(&key), Some(&CloudState::Available));
            assert!(shell.cloud_refresh_again);
            // The menu entry is gone at once.
            assert_eq!(resume_label(shell, id, cx), None);
            // The in-flight refresh lands with the old Stopped: dropped, and read again.
            shell.finish_cloud_refresh(stale, cx);
            assert_eq!(shell.cloud_states.get(&key), Some(&CloudState::Available));
            assert!(!shell.cloud_refresh_again);
            assert!(shell.cloud_refreshing, "a new read started");
            assert_eq!(resume_label(shell, id, cx), None);
        })
        .unwrap();
}

/// Runs `sessions.archive` with `cloud` on session `id` and returns the app call's answer.
fn app_archive(
    window: gpui::WindowHandle<Shell>,
    cx: &mut TestAppContext,
    id: i64,
    choice: &str,
) -> Result<Value, String> {
    app_archive_full(window, cx, id, choice).map_err(|failure| failure.message)
}

fn app_archive_full(
    window: gpui::WindowHandle<Shell>,
    cx: &mut TestAppContext,
    id: i64,
    choice: &str,
) -> Result<Value, super::cloud_app_archive::ArchiveFailure> {
    let (reply, answer) = async_channel::bounded(1);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, window, cx| {
            shell.app_archive_cloud(id, choice, reply, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    answer.try_recv().expect("the app call was answered")
}

/// `fake_convert` that counts its calls.
fn counting_convert(
    store: Database,
    fail: Option<&'static str>,
) -> (cloud::ConvertFn, Arc<Mutex<u32>>) {
    let inner = fake_convert(store, fail);
    let count = Arc::new(Mutex::new(0));
    let seen = count.clone();
    (
        Arc::new(move |svc, id, files, progress| {
            *seen.lock().unwrap() += 1;
            inner(svc, id, files, progress)
        }),
        count,
    )
}

#[gpui::test]
fn app_archive_of_a_codespace_session_converts_then_acts_once(cx: &mut TestAppContext) {
    for (choice, expected) in [
        ("keep", vec![]),
        ("stop", vec!["codespace stop -c octo-abc"]),
        ("delete", vec!["codespace delete -c octo-abc --force"]),
    ] {
        let (window, id, gh) = shell_with_cloud(cx);
        let (convert, count) = window
            .update(cx, |shell, _, _| {
                let (convert, count) = counting_convert(shell.store.clone(), None);
                shell.cloud_convert = convert.clone();
                (convert, count)
            })
            .unwrap();
        drop(convert);
        let value = app_archive(window, cx, id, choice).unwrap();
        assert_eq!(*count.lock().unwrap(), 1, "{choice}");
        assert_eq!(gh.archive_calls(), expected, "{choice}");
        assert_eq!(value["sessionId"], id);
        assert_eq!(value["archived"], true);
        assert_eq!(value["cloud"]["kind"], "codespace");
        assert_eq!(value["cloud"]["name"], "octo-abc");
        assert_eq!(value["cloud"]["action"], choice);
        assert_eq!(value["cloud"]["done"], true);
        window
            .update(cx, |shell, _, _| {
                // The cloud row is replaced by one archived local copy, as the dialog does.
                assert!(!shell.is_cloud(id), "{choice}");
                assert_eq!(archived_locals(shell).len(), 1, "{choice}");
                assert!(shell.cloud_busy.is_empty());
            })
            .unwrap();
    }
}

#[gpui::test]
fn app_archive_of_a_sandbox_session_converts_then_acts_once(cx: &mut TestAppContext) {
    for (choice, verb) in [
        ("keep", None),
        ("stop", Some(" stop ")),
        ("delete", Some(" rm ")),
    ] {
        let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
        let count = window
            .update(cx, |shell, _, _| {
                let (convert, count) = counting_convert(shell.store.clone(), None);
                shell.cloud_convert = convert;
                count
            })
            .unwrap();
        let value = app_archive(window, cx, id, choice).unwrap();
        assert_eq!(*count.lock().unwrap(), 1, "{choice}");
        let changes = sbx.changes();
        match verb {
            None => assert!(changes.is_empty(), "{changes:?}"),
            Some(verb) => {
                assert_eq!(changes.len(), 1, "{choice}: {changes:?}");
                assert!(changes[0].contains(verb), "{choice}: {changes:?}");
            }
        }
        assert_eq!(value["cloud"]["kind"], "sandbox");
        assert_eq!(value["cloud"]["name"], SBX_NAME);
        assert_eq!(value["cloud"]["action"], choice);
        assert_eq!(value["cloud"]["done"], true);
        assert_eq!(value["archived"], true);
    }
}

#[gpui::test]
fn app_archive_keeps_the_workspace_when_the_conversion_fails(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, _| {
            shell.cloud_convert = fake_convert(shell.store.clone(), Some("copy failed"));
        })
        .unwrap();
    for choice in ["stop", "delete"] {
        let error = app_archive(window, cx, id, choice).unwrap_err();
        assert!(error.contains("copy failed"), "{error}");
        assert!(error.contains("was kept"), "{error}");
        assert!(gh.archive_calls().is_empty(), "{choice}");
    }
    window
        .update(cx, |shell, _, _| {
            assert!(shell.is_cloud(id) && shell.cloud_busy.is_empty());
            assert!(archived_locals(shell).is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn app_archive_keeps_the_sandbox_when_its_conversation_cannot_be_reached(cx: &mut TestAppContext) {
    let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
    window
        .update(cx, |shell, _, _| {
            shell.cloud_convert = history_lost_convert(shell.store.clone(), true);
        })
        .unwrap();
    let error = app_archive(window, cx, id, "delete").unwrap_err();
    assert!(error.contains("could not be reached"), "{error}");
    assert!(error.contains("was kept"), "{error}");
    assert!(sbx.changes().is_empty(), "{:?}", sbx.calls());
    window
        .update(cx, |shell, _, _| assert!(shell.is_cloud(id)))
        .unwrap();
}

#[gpui::test]
fn app_archive_does_not_stop_or_delete_after_a_copy_without_the_conversation(
    cx: &mut TestAppContext,
) {
    for choice in ["stop", "delete"] {
        let (window, id, sbx) = shell_with_sandbox(cx, "running", CloudState::Available, true);
        window
            .update(cx, |shell, _, _| {
                shell.cloud_convert = history_lost_convert(shell.store.clone(), false);
            })
            .unwrap();
        let error = app_archive(window, cx, id, choice).unwrap_err();
        assert!(error.contains("conversation"), "{error}");
        assert!(error.contains("not stopped or deleted"), "{error}");
        assert!(sbx.changes().is_empty(), "{choice}: {:?}", sbx.calls());
        window
            .update(cx, |shell, _, _| {
                assert!(!shell.is_cloud(id), "the code copy is archived");
                assert_eq!(archived_locals(shell).len(), 1);
            })
            .unwrap();
    }
}

#[gpui::test]
fn app_archive_never_deletes_a_workspace_another_session_still_uses(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    let count = window
        .update(cx, |shell, _, _| {
            cloud_row(shell, "octo-abc");
            let (convert, count) = counting_convert(shell.store.clone(), None);
            shell.cloud_convert = convert;
            count
        })
        .unwrap();
    let error = app_archive(window, cx, id, "delete").unwrap_err();
    assert!(error.contains("still use"), "{error}");
    assert_eq!(*count.lock().unwrap(), 0);
    assert!(gh.archive_calls().is_empty());
    window
        .update(cx, |shell, _, _| assert!(shell.is_cloud(id)))
        .unwrap();
    // Stopping is still allowed: it keeps everything and is what the dialog offers too.
    let value = app_archive(window, cx, id, "stop").unwrap();
    assert_eq!(value["cloud"]["action"], "stop");
    assert_eq!(gh.archive_calls(), ["codespace stop -c octo-abc"]);
}

#[gpui::test]
fn app_archive_refuses_cloud_on_a_local_session_and_an_unknown_value(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    let local = window
        .update(cx, |shell, _, _| {
            let local = locked(&shell.store)
                .unwrap()
                .create("claude", "/work/monocode")
                .unwrap();
            local.id
        })
        .unwrap();
    let error = app_archive(window, cx, local, "stop").unwrap_err();
    assert!(error.contains("only applies to"), "{error}");
    assert!(app_archive(window, cx, id, "remove").is_err());
    assert!(gh.calls().is_empty());
}

#[gpui::test]
fn app_archive_keep_does_not_mark_the_workspace_as_stopping(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    app_archive(window, cx, id, "keep").unwrap();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(
                shell.cloud_states.get("octo-abc"),
                Some(&CloudState::Stopped)
            );
        })
        .unwrap();
    let (window, id, _) = shell_with_cloud(cx);
    app_archive(window, cx, id, "stop").unwrap();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(
                shell.cloud_states.get("octo-abc"),
                Some(&CloudState::Stopping)
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_second_archive_of_a_busy_workspace_is_refused_everywhere(cx: &mut TestAppContext) {
    let (window, a, gh) = shell_with_cloud(cx);
    let b = window
        .update(cx, |shell, _, _| cloud_row(shell, "octo-abc"))
        .unwrap();
    let count = window
        .update(cx, |shell, _, _| {
            let (convert, count) = counting_convert(shell.store.clone(), None);
            shell.cloud_convert = convert;
            count
        })
        .unwrap();
    let (reply_a, answer_a) = async_channel::bounded(1);
    let (reply_b, answer_b) = async_channel::bounded(1);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, window, cx| {
            shell.app_archive_cloud(a, "stop", reply_a, window, cx);
            // The sibling, the dialog, the session menu and the remove prompt all wait.
            shell.app_archive_cloud(b, "stop", reply_b, window, cx);
            shell.open_cloud_archive(vec![b], window, cx);
            assert!(shell.cloud_archive.is_none());
            shell.run_workspace_action(
                CloudRef::codespace("octo-abc"),
                ArchiveChoice::Delete,
                window,
                cx,
            );
            assert!(shell.cloud_archive.is_none());
            shell.confirm_remove_cloud_session(b, window, cx);
            assert!(shell.cloud_archive.is_none());
        })
        .unwrap();
    let refused = answer_b.try_recv().unwrap().unwrap_err();
    assert!(
        refused.message.contains("still running"),
        "{}",
        refused.message
    );
    assert!(!refused.archived);
    visual.run_until_parked();
    assert!(answer_a.try_recv().unwrap().is_ok());
    assert_eq!(*count.lock().unwrap(), 1);
    assert_eq!(gh.archive_calls(), ["codespace stop -c octo-abc"]);
    window
        .update(visual, |shell, _, _| {
            assert!(shell.cloud_workspaces_busy.is_empty())
        })
        .unwrap();
}

#[gpui::test]
fn an_app_archive_leaves_a_dialog_the_user_opened_meanwhile_alone(cx: &mut TestAppContext) {
    let (window, a, gh) = shell_with_cloud(cx);
    let other = window
        .update(cx, |shell, _, _| cloud_row(shell, "octo-xyz"))
        .unwrap();
    let (reply, answer) = async_channel::bounded(1);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, window, cx| {
            shell.app_archive_cloud(a, "delete", reply, window, cx);
            shell.open_cloud_archive(vec![other], window, cx);
            assert!(shell.cloud_archive.is_some());
        })
        .unwrap();
    visual.run_until_parked();
    assert!(answer.try_recv().unwrap().is_ok());
    assert_eq!(gh.archive_calls(), ["codespace delete -c octo-abc --force"]);
    window
        .update(visual, |shell, _, _| {
            let dialog = shell
                .cloud_archive
                .as_ref()
                .expect("the user's dialog stays");
            assert_eq!(dialog.ids(), vec![other]);
            assert!(matches!(dialog.phase, cloud::Phase::Choose));
        })
        .unwrap();
}

#[gpui::test]
fn messages_to_a_session_being_archived_are_refused(cx: &mut TestAppContext) {
    let (window, id, _) = shell_with_cloud(cx);
    let (reply, _answer) = async_channel::bounded(1);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, window, cx| {
            shell.app_archive_cloud(id, "stop", reply, window, cx);
            assert!(shell.cloud_archive_in_progress(id));
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(!shell.cloud_archive_in_progress(id))
        })
        .unwrap();
}

#[gpui::test]
fn a_partly_saved_archive_reports_the_session_as_archived(cx: &mut TestAppContext) {
    // Delete converts the target first, then its sibling; the sibling fails.
    let (window, a, gh) = shell_with_cloud(cx);
    let b = window
        .update(cx, |shell, _, _| {
            let b = cloud_row(shell, "octo-abc");
            // Archived, so it does not block the delete but is still converted.
            locked(&shell.store).unwrap().set_archived(b, true).unwrap();
            if let Some(summary) = shell.session_summaries.get_mut(&b) {
                summary.archived = true;
            }
            let inner = fake_convert(shell.store.clone(), None);
            shell.cloud_convert = Arc::new(move |svc, id, files, progress| {
                if id == b {
                    return Err(ConvertError::Failed("sibling copy failed".into()));
                }
                inner(svc, id, files, progress)
            });
            b
        })
        .unwrap();
    let _ = b;
    let result = app_archive_full(window, cx, a, "delete");
    // The target was saved: whatever the outcome, it is not reported as untouched.
    match result {
        Ok(value) => {
            assert_eq!(value["archived"], true);
            assert_eq!(value["cloud"]["done"], false);
        }
        Err(failure) => assert!(failure.archived, "{}", failure.message),
    }
    assert!(gh.archive_calls().is_empty(), "{:?}", gh.archive_calls());
}

#[gpui::test]
fn app_archive_delete_review_error_tells_the_user_to_review_in_the_app(cx: &mut TestAppContext) {
    let (window, id, gh) = shell_with_cloud(cx);
    window
        .update(cx, |shell, _, _| {
            let inner = fake_convert(shell.store.clone(), None);
            shell.cloud_convert = Arc::new(move |svc, session, files, progress| {
                let mut done = inner(svc, session, files, progress)?;
                done.ignored = vec![(4, ".env.local".into())];
                Ok(done)
            });
        })
        .unwrap();
    let failure = app_archive_full(window, cx, id, "delete").unwrap_err();
    assert!(
        failure.message.contains("review them in the app"),
        "{}",
        failure.message
    );
    assert!(failure.archived);
    assert!(gh.archive_calls().is_empty());
}
