//! View-model and flow tests for cloud sessions, with a fake `gh` and a fake host.
use super::cloud::{self, ArchiveChoice, CloudMark, Then, Tone};
use super::cloud_picker;
use super::*;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{CloudSessions, CloudState, CodespaceChoice, Host},
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
    assert_eq!((then, calls), (Then::Archive, vec![]));
    let (then, calls) = run(ArchiveChoice::Stop);
    assert_eq!(then, Then::Archive);
    assert_eq!(calls, vec!["codespace stop -c octo-abc".to_string()]);
    let (then, calls) = run(ArchiveChoice::Delete);
    assert_eq!(then, Then::Forget);
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
            assert_eq!(dialog.id, id);
            assert!(!shell.session_summaries[&id].archived, "not archived yet");
        })
        .unwrap();
    assert!(visual.debug_bounds("cloud-archive-stop").is_some());
    assert!(visual.debug_bounds("cloud-archive-delete").is_some());
    assert!(visual.debug_bounds("cloud-archive-keep").is_some());
    window
        .update(visual, |shell, window, cx| {
            shell.confirm_cloud_archive(ArchiveChoice::Stop, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(gh.calls(), ["codespace stop -c octo-abc"]);
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.cloud_archive.is_none());
            assert!(shell.session_metadata(id, cx).summary.archived);
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
            shell.open_cloud_archive(id, window, cx);
            assert_eq!(shell.cloud_archive.as_ref().unwrap().others, 1);
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
            shell.open_cloud_archive(id, window, cx);
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
