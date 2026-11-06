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
            .unwrap_or("");
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
            warnings: vec![],
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
fn the_new_session_flow_prepares_the_target_then_starts_a_claude_session() {
    let checkout = github_checkout();
    let local = checkout.path().to_string_lossy().into_owned();
    let gh = FakeGh::new(vec![
        ("auth status", AUTH_OK),
        ("codespace list", LIST),
        ("codespace create", "octo-new1\n"),
        ("codespace view", r#"{"state":"Available"}"#),
    ]);
    let host = Arc::new(FakeHost::default());
    let store = memory_store();
    let svc = service(&gh, &host, store.clone());
    let target =
        cloud_picker::target_for(CodespaceChoice::New, " feature ", Some("basicLinux32gb"))
            .unwrap();
    assert_eq!(target.branch.as_deref(), Some("feature"));
    let mut lines = vec![];
    let (summary, binding) =
        cloud_picker::start_cloud_session(&svc, &local, target, &mut |l| lines.push(l.to_string()))
            .unwrap();
    assert_eq!(summary.provider, "claude");
    assert_eq!(summary.cwd, binding.project_key);
    assert_eq!(binding.codespace, "octo-new1");
    assert!(
        gh.calls()
            .iter()
            .any(|c| c.contains("--branch feature") && c.contains("--machine basicLinux32gb"))
    );
    assert!(
        lines.iter().any(|l| l == "Pairing with the codespace"),
        "{lines:?}"
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
    cloud_picker::start_cloud_session(&svc, &local, target, &mut |_| {}).unwrap();
    assert!(!gh.calls().iter().any(|c| c.contains("codespace create")));
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
    assert_eq!(gh.calls(), ["codespace delete -c octo-abc --force"]);
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
    assert!(gh.calls().is_empty(), "nothing deleted: {:?}", gh.calls());
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
            shell.run_cloud_archive(ArchiveChoice::Delete, true, window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(gh.calls(), ["codespace delete -c octo-abc --force"]);
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
            shell.run_cloud_archive(ArchiveChoice::Stop, false, window, cx)
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
            shell.run_cloud_archive(ArchiveChoice::Stop, true, window, cx);
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
    assert_eq!(gh.calls(), ["codespace delete -c octo-abc --force"]);
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
        &mut |_| {},
    );
    assert_eq!(out[0].result.as_ref().unwrap_err(), cloud::CANCELLED);
    assert!(out[0].converted.is_empty());
    assert!(gh.calls().is_empty());
}
