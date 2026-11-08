use super::cloud_target::*;
use super::*;
use gpui::{TestAppContext, WindowHandle};
use monocode_core::cloud_sessions::docker_config::{self, DockerProjectConfig};
use monocode_core::store::Store;
use std::sync::Mutex;

const PROJECT: &str = "/work/widgets";

fn conversation(
    cx: &mut TestAppContext,
    cwd: &str,
    has_messages: bool,
) -> WindowHandle<Conversation> {
    cx.update(|cx| cx.set_reduce_motion(true));
    let store = Store::open(Path::new(":memory:")).unwrap();
    let mut summary = store.create("claude", PROJECT).unwrap();
    summary.cwd = cwd.into();
    summary.has_messages = has_messages;
    cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view
            })
        })
        .unwrap()
    })
}

fn save_config(view: &Conversation) -> DockerProjectConfig {
    let config = DockerProjectConfig {
        repo_url: "https://github.com/octo/widgets.git".into(),
        base_ref: "develop".into(),
        ..Default::default()
    };
    let saved =
        docker_config::save(&locked(&view.store).unwrap(), &view.summary.cwd, &config).unwrap();
    view.forget_docker_config();
    saved
}

/// `(id, label, checked, description)` of the action rows after the section header.
fn rows(view: &Conversation) -> Vec<(String, String, bool, Option<String>)> {
    view.cloud_menu_items()
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Action(a) => Some((
                a.id.to_string(),
                a.label.to_string(),
                a.checked,
                a.description.map(|d| d.to_string()),
            )),
            _ => None,
        })
        .collect()
}

fn header(view: &Conversation) -> Option<String> {
    view.cloud_menu_items()
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Header(text) => Some(text.to_string()),
            _ => None,
        })
}

#[gpui::test]
fn the_menu_lists_cloud_rows_with_their_gates(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            // A project that is not a GitHub repository offers Docker Sandbox only, and says
            // it needs setup while no config is saved.
            assert_eq!(header(view).as_deref(), Some("Run on cloud"));
            assert_eq!(
                rows(view),
                vec![(
                    "workspace:cloud-docker".to_string(),
                    "Docker Sandbox".to_string(),
                    false,
                    Some("Set up first".to_string())
                )]
            );
            view.set_offers_codespace(true, cx);
            let labels: Vec<_> = rows(view).into_iter().map(|row| row.1).collect();
            assert_eq!(labels, ["Codespace", "Docker Sandbox"]);
            save_config(view);
            assert_eq!(rows(view)[1].3, None);
        })
        .unwrap();
}

#[gpui::test]
fn no_cloud_rows_for_a_remote_project_or_after_the_first_message(cx: &mut TestAppContext) {
    let remote = conversation(cx, "remote://env-octo-abc/workspaces/widgets", false);
    remote
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            assert!(view.cloud_menu_items().is_empty());
        })
        .unwrap();
    let started = conversation(cx, PROJECT, true);
    started
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            assert!(view.cloud_menu_items().is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn docker_without_a_config_opens_the_setup_dialog_instead_of_selecting(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let sink = opened.clone();
    window
        .update(cx, |view, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, event: &OpenDockerSetup, _| {
                sink.lock().unwrap().push(event.0.clone());
            })
            .detach();
            assert!(view.cloud_menu_choice("workspace:cloud-docker", cx));
            assert_eq!(view.cloud_target(), None);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(*opened.lock().unwrap(), [PROJECT.to_string()]);
}

#[gpui::test]
fn codespace_is_refused_when_the_project_does_not_offer_it(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            assert!(view.cloud_menu_choice("workspace:cloud-codespace", cx));
            assert_eq!(view.cloud_target(), None);
        })
        .unwrap();
}

#[gpui::test]
fn choosing_a_target_changes_the_triggers_and_shows_the_cloud_row(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let draw = |visual: &mut gpui::VisualTestContext| {
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    draw(visual);
    assert!(visual.debug_bounds("cloud-target-row").is_none());
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert_eq!(view.cloud_target(), Some(CloudTarget::DockerSandbox));
            assert_eq!(
                view.cloud_workspace_label().as_deref(),
                Some("\u{2601} Docker Sandbox")
            );
            assert_eq!(view.cloud_base_label().as_deref(), Some("base develop"));
            let checked: Vec<_> = rows(view).into_iter().map(|row| row.2).collect();
            assert_eq!(checked, [true]);
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-target-row").is_some());
    assert!(visual.debug_bounds("cloud-target-create").is_some());
    window
        .update(visual, |view, _, cx| {
            // Local checkouts mean Local again.
            view.cloud_menu_choice("workspace:current", cx);
            assert_eq!(view.cloud_target(), None);
            assert_eq!(view.cloud_workspace_label(), None);
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-target-row").is_none());
}

#[gpui::test]
fn send_is_disabled_until_the_session_is_created(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.input
                .update(cx, |input, cx| input.set_text("fix it".into(), cx));
            assert!(view.cloud_create_pending());
            view.send(cx);
            assert_eq!(view.status, CREATE_FIRST);
            // Nothing was sent or taken from the composer.
            assert_eq!(view.input.read(cx).content.as_ref(), "fix it");
            assert!(!view.busy);
            view.clear_cloud_target(cx);
            assert!(!view.cloud_create_pending());
        })
        .unwrap();
}

#[gpui::test]
fn create_and_open_asks_the_shell_with_the_config_branch_and_prompt(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let sink = requests.clone();
    window
        .update(cx, |view, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, event: &CreateCloudSession, _| {
                sink.lock().unwrap().push(event.0.clone());
            })
            .detach();
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.input
                .update(cx, |input, cx| input.set_text("  fix it  ".into(), cx));
            let branch = view.cloud_draft.as_ref().unwrap().branch.clone();
            assert!(branch.read(cx).content.starts_with("monoptah/"));
            branch.update(cx, |input, cx| input.set_text("feature/x".into(), cx));
            view.create_cloud(cx);
            assert!(view.cloud_draft.as_ref().unwrap().creating);
            // A second press while creating asks for nothing more.
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].target, CloudTarget::DockerSandbox);
    assert_eq!(sent[0].project, PROJECT);
    assert_eq!(sent[0].branch, "feature/x");
    assert_eq!(sent[0].prompt.as_deref(), Some("fix it"));
    assert_eq!(sent[0].config.as_ref().unwrap().base_ref, "develop");
    window
        .update(cx, |view, _, cx| {
            // A refusal shows inline and allows another try.
            let id = sent[0].request_id;
            view.cloud_create_finished(id, Err("sbx is not installed".into()), cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert!(!draft.creating);
            assert_eq!(draft.error.as_deref(), Some("sbx is not installed"));
            // Created: the text moved to the queue and the target is gone.
            view.create_cloud(cx);
            let again = view.cloud_draft.as_ref().unwrap().request;
            assert_ne!(again, id);
            view.cloud_create_finished(again, Ok(true), cx);
            assert!(view.cloud_draft.is_none());
            assert_eq!(view.input.read(cx).content.as_ref(), "");
        })
        .unwrap();
}

#[gpui::test]
fn create_and_open_validates_before_asking(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = Arc::new(Mutex::new(0));
    let sink = requests.clone();
    window
        .update(cx, |view, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, _: &CreateCloudSession, _| {
                *sink.lock().unwrap() += 1;
            })
            .detach();
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let branch = view.cloud_draft.as_ref().unwrap().branch.clone();
            branch.update(cx, |input, cx| input.set_text("  ".into(), cx));
            view.create_cloud(cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert!(!draft.creating);
            assert_eq!(
                draft.error.as_deref(),
                Some("Enter a branch name for the new session.")
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(*requests.lock().unwrap(), 0);
}

#[test]
fn memory_and_compute_labels() {
    assert_eq!(memory_label("4g"), "4 GiB");
    assert_eq!(memory_label("512m"), "512 MiB");
    assert_eq!(memory_label("lots"), "lots");
    assert_eq!(
        compute_label(&DockerProjectConfig::default()),
        "2 vCPU / 4 GiB \u{b7} 2h then stop"
    );
}

fn temp_repo(head: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "monoptah-d2b-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/HEAD"), head).unwrap();
    dir.to_string_lossy().into_owned()
}

/// Requests sent by "Create and open" on `window`.
fn watch(
    window: WindowHandle<Conversation>,
    cx: &mut TestAppContext,
) -> Arc<Mutex<Vec<CloudCreate>>> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let sink = requests.clone();
    window
        .update(cx, |_, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, event: &CreateCloudSession, _| {
                sink.lock().unwrap().push(event.0.clone());
            })
            .detach();
        })
        .unwrap();
    requests
}

#[gpui::test]
fn the_workspace_cannot_change_while_a_create_runs(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.create_cloud(cx);
            assert!(view.cloud_creating());
            let request = view.cloud_draft.as_ref().unwrap().request;
            // Local rows are refused: the prompt must not run twice.
            assert!(!view.cloud_menu_choice("workspace:current", cx) || view.cloud_creating());
            assert!(view.cloud_menu_choice("workspace:new", cx));
            view.clear_cloud_target(cx);
            assert!(!view.use_existing_worktree("/work/other", cx));
            // Another target does not make a second draft.
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.target, CloudTarget::DockerSandbox);
            assert_eq!(draft.request, request);
            assert!(draft.creating);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[gpui::test]
fn an_answer_for_another_request_is_ignored(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.input
                .update(cx, |input, cx| input.set_text("keep me".into(), cx));
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let id = requests.lock().unwrap()[0].request_id;
    window
        .update(cx, |view, _, cx| {
            view.cloud_create_finished(id + 1, Ok(true), cx);
            assert!(view.cloud_creating());
            assert_eq!(view.input.read(cx).content.as_ref(), "keep me");
            view.cloud_create_finished(id, Ok(true), cx);
            assert!(view.cloud_draft.is_none());
            // With no draft left, a late answer changes nothing.
            view.input
                .update(cx, |input, cx| input.set_text("local".into(), cx));
            view.cloud_create_finished(id, Ok(true), cx);
            assert_eq!(view.input.read(cx).content.as_ref(), "local");
        })
        .unwrap();
}

#[gpui::test]
fn enter_in_the_branch_field_does_not_create(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let branch = view.cloud_draft.as_ref().unwrap().branch.clone();
            branch.update(cx, |_, cx| cx.emit(input::Submitted));
            assert!(!view.cloud_creating());
        })
        .unwrap();
    cx.run_until_parked();
    assert!(requests.lock().unwrap().is_empty());
}

#[gpui::test]
fn codespace_uses_the_project_head_and_refuses_a_detached_one(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            // A worktree choice is dropped, and its branch is never used.
            view.settings.new_worktree = true;
            view.settings.worktree_cwd = Some("/work/wt".into());
            view.branch = Some("wt-branch".into());
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            assert!(!view.settings.new_worktree && view.settings.worktree_cwd.is_none());
            view.branch = Some("wt-branch".into());
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    {
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].target, CloudTarget::Codespace);
        assert_eq!(sent[0].branch, "main");
    }
    let detached = temp_repo("0123456789abcdef\n");
    let window = conversation(cx, &detached, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            view.create_cloud(cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert!(!draft.creating);
            assert!(draft.error.as_deref().unwrap().contains("detached"));
        })
        .unwrap();
    cx.run_until_parked();
    assert!(requests.lock().unwrap().is_empty());
}

#[gpui::test]
fn a_created_codespace_error_locks_the_button(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let id = requests.lock().unwrap()[0].request_id;
    let note = monocode_core::cloud_sessions::created_note("octo-abc", "store failed");
    assert_eq!(created_codespace(&note), Some("octo-abc"));
    assert_eq!(created_codespace("Could not create it"), None);
    window
        .update(cx, |view, _, cx| {
            view.cloud_create_finished(id, Err(note), cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.created.as_deref(), Some("octo-abc"));
            assert!(draft.error.as_deref().unwrap().contains("octo-abc"));
            // No plain re-create.
            view.create_cloud(cx);
            assert!(!view.cloud_creating());
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[gpui::test]
fn the_docker_config_is_read_once_per_menu_opening(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, _| {
            assert!(rows(view)[0].3.is_some(), "no config yet");
            // Saved behind the cache's back (the setup dialog): not seen until reopened.
            let config = DockerProjectConfig {
                repo_url: "https://github.com/octo/widgets.git".into(),
                ..Default::default()
            };
            docker_config::save(&locked(&view.store).unwrap(), PROJECT, &config).unwrap();
            assert!(rows(view)[0].3.is_some());
            view.forget_docker_config();
            assert!(rows(view)[0].3.is_none());
        })
        .unwrap();
}
