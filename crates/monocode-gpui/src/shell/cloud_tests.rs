//! View-model and flow tests for cloud sessions, with a fake `gh` and a fake host.
use super::cloud::{self, ArchiveChoice, CloudMark, Then, Tone};
use super::cloud_picker;
use super::*;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{CloudSessions, CloudState, CodespaceChoice, Converted, Host, Resume},
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
    Arc::new(move |_, id, progress| {
        progress("Waking the codespace");
        if let Some(error) = fail {
            return Err(error.into());
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
            codespace: remote.project.codespace.clone().unwrap(),
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
    assert_eq!(cloud::others_note(0), None);
    assert_eq!(
        cloud::others_note(1).as_deref(),
        Some("1 other session uses this codespace.")
    );
    assert_eq!(
        cloud::others_note(3).as_deref(),
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
        let then = cloud::apply_choice(&svc, choice, "octo-abc").unwrap();
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
    let error = cloud::apply_choice(&svc, ArchiveChoice::Stop, "octo-abc").unwrap_err();
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
            codespace: "octo-abc".into()
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
            assert_eq!(shell.active_codespace().as_deref(), Some("octo-abc"));
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
            shell.cloud_convert = Arc::new(move |cloud, id, progress| {
                log.lock().unwrap().push(id);
                if id == b {
                    Err("disk full".into())
                } else {
                    inner(cloud, id, progress)
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
            shell.cloud_convert = Arc::new(move |cloud, id, progress| {
                log.lock().unwrap().push(id);
                inner(cloud, id, progress)
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
            shell.cloud_convert = Arc::new(move |svc, id, progress| {
                let mut done = inner(svc, id, progress)?;
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
    Arc::new(move |svc, id, progress| {
        let mut done = inner(svc, id, progress)?;
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
        let mut done = fake_convert(store.clone(), None)(&svc, id, &mut |_| {}).unwrap();
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
    let (then, warning) = cloud::apply_choice(&svc, ArchiveChoice::Delete, "octo-abc").unwrap();
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
            shell.cloud_convert = Arc::new(move |cloud, id, progress| {
                log.lock().unwrap().push(id);
                inner(cloud, id, progress)
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
            assert_eq!(chat.read(cx).cloud_queued(), Some("fix the bug"));
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
            shell.finish_ready(id, session, window, cx);
            let chat = shell.sessions[&id].read(cx);
            assert_eq!(chat.summary.cwd, cwd);
            assert!(!chat.cloud_start_blocks());
            assert_eq!(chat.cloud_start().unwrap().phase, StartPhase::Ready);
            assert!(chat.cloud_start().unwrap().ready.is_some());
            assert!(
                chat.remote_debug().contains("waiting=true"),
                "{}",
                chat.remote_debug()
            );
            assert_eq!(shell.session_summaries[&id].cwd, cwd);
            assert!(shell.cloud_start_label(id).is_none());
            assert!(
                shell
                    .mono_session_views(cx)
                    .iter()
                    .find(|session| session.id == id)
                    .unwrap()
                    .busy
            );
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
                Arc::new(|_, _, _| panic!("must not back up a missing codespace"));
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
                        Arc::new(|_, _, _| panic!("lookup failure must not attempt backup"));
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
