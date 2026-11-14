use super::cloud_branches::*;
use super::cloud_target::*;
use super::*;
use gpui::{TestAppContext, WindowHandle};
use monocode_core::cloud_sessions::CloudRef;
use monocode_core::cloud_sessions::docker_config::{self, DockerProjectConfig};
use monocode_core::codespaces;
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
    // What the shell does when the project's Cloud settings are saved.
    view.forget_project_cloud();
    saved
}

/// `(id, label, checked, description)` of the backend rows (Codespace, Docker Sandbox) after
/// the section header.
fn rows(view: &Conversation) -> Vec<(String, String, bool, Option<String>)> {
    all_rows(view)
        .into_iter()
        .filter(|row| {
            matches!(
                row.0.as_str(),
                "workspace:cloud-codespace" | "workspace:cloud-docker"
            )
        })
        .collect()
}

/// `(id, label, checked, description)` of every action row of the section.
fn all_rows(view: &Conversation) -> Vec<(String, String, bool, Option<String>)> {
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
                    Some("Paid compute in your Docker account \u{b7} Set up first".to_string())
                )]
            );
            view.set_offers_codespace(true, cx);
            let labels: Vec<_> = rows(view).into_iter().map(|row| row.1).collect();
            assert_eq!(labels, ["Codespace", "Docker Sandbox"]);
            save_config(view);
            assert_eq!(
                rows(view)[1].3.as_deref(),
                Some("Paid compute in your Docker account")
            );
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
    assert!(visual.debug_bounds("cloud-target-panel").is_none());
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
    assert!(visual.debug_bounds("cloud-target-panel").is_some());
    assert!(visual.debug_bounds("cloud-target-create").is_none());
    window
        .update(visual, |view, _, cx| {
            // Local checkouts mean Local again.
            view.cloud_menu_choice("workspace:current", cx);
            assert_eq!(view.cloud_target(), None);
            assert_eq!(view.cloud_workspace_label(), None);
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-target-panel").is_none());
}

/// Presses Enter in the composer, as its own update; callers run `run_until_parked` for the subscription.
fn press_enter(window: WindowHandle<Conversation>, cx: &mut impl gpui::AppContext) {
    window
        .update(cx, |view, _, cx| {
            view.input.update(cx, |_, cx| cx.emit(input::Submitted))
        })
        .unwrap();
}

/// The composer's own send (Enter or the button), as opposed to a Mono or API send.
fn user_send(view: &mut Conversation, cx: &mut Context<Conversation>) {
    view.user_send = true;
    view.send(cx);
    view.user_send = false;
}

fn type_prompt(view: &Conversation, text: &str, cx: &mut Context<Conversation>) {
    view.input
        .update(cx, |input, cx| input.set_text(text.into(), cx));
}

#[gpui::test]
fn enter_with_text_creates_once_and_sends_the_prompt_with_the_request(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            type_prompt(view, "  fix it  ", cx);
        })
        .unwrap();
    press_enter(window, cx);
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert!(view.cloud_creating()))
        .unwrap();
    // Enter auto-repeat and Enter plus click: still one request.
    press_enter(window, cx);
    cx.run_until_parked();
    press_enter(window, cx);
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            user_send(view, cx);
            // Nothing was sent locally: the prompt left the composer for the queue shelf and
            // waits there, once, until the answer.
            assert!(!view.busy);
            assert_eq!(view.input.read(cx).content.as_ref(), "");
            let queued: Vec<_> = view
                .follow_ups
                .state
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect();
            assert_eq!(queued, vec!["fix it"]);
            view.pump_follow_ups(cx);
            assert!(view.launched.is_empty());
        })
        .unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt.as_deref(), Some("fix it"));
    assert_eq!(requests[0].target, CloudTarget::DockerSandbox);
}

#[gpui::test]
fn the_send_button_creates_once_and_a_double_click_does_not_repeat(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            type_prompt(view, "fix it", cx);
        })
        .unwrap();
    draw(visual);
    let send = visual.debug_bounds("send-or-stop").unwrap();
    visual.simulate_click(send.center(), Default::default());
    draw(visual);
    visual.simulate_click(send.center(), Default::default());
    visual.simulate_click(send.center(), Default::default());
    draw(visual);
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(
        requests.lock().unwrap()[0].prompt.as_deref(),
        Some("fix it")
    );
}

#[gpui::test]
fn send_with_an_empty_prompt_is_disabled_and_names_the_noun(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let tip = |view: &Conversation, has_value| view.cloud_send_tip(has_value);
            let (text, usable) = tip(view, false).unwrap();
            assert_eq!(
                text.as_ref(),
                "Type a prompt to create the Docker sandbox and send it"
            );
            assert!(!usable);
            let (text, usable) = tip(view, true).unwrap();
            assert_eq!(text.as_ref(), "Create Docker sandbox and send");
            assert!(usable);
            // Whitespace is empty, and Enter on it creates nothing.
            type_prompt(view, "   ", cx);
            user_send(view, cx);
            assert!(!view.cloud_creating());
            view.clear_cloud_target(cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            let (text, _) = tip(view, true).unwrap();
            assert_eq!(text.as_ref(), "Create codespace and send");
            let (text, _) = tip(view, false).unwrap();
            assert_eq!(
                text.as_ref(),
                "Type a prompt to create the codespace and send it"
            );
        })
        .unwrap();
    draw(visual);
    let send = visual.debug_bounds("send-or-stop").unwrap();
    visual.simulate_click(send.center(), Default::default());
    draw(visual);
    assert!(requests.lock().unwrap().is_empty());
    window
        .update(visual, |view, _, cx| {
            type_prompt(view, "go", cx);
            user_send(view, cx);
            let (text, usable) = view.cloud_send_tip(true).unwrap();
            assert_eq!(text.as_ref(), "Creating codespace\u{2026}");
            assert!(!usable);
        })
        .unwrap();
}

#[gpui::test]
fn a_failed_create_keeps_the_text_and_shows_the_error_then_send_works_again(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            type_prompt(view, "fix it", cx);
        })
        .unwrap();
    press_enter(window, visual);
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            let id = view.cloud_draft.as_ref().unwrap().request;
            view.cloud_create_finished(id, Err("Docker refused".into()), cx);
            assert_eq!(view.input.read(cx).content.as_ref(), "fix it");
            assert!(!view.cloud_creating());
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-target-error").is_some());
    press_enter(window, visual);
    visual.run_until_parked();
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[gpui::test]
fn a_validation_error_from_send_is_inline_and_asks_for_nothing(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            set_text(view, |d| d.branch.clone(), "", cx);
            type_prompt(view, "fix it", cx);
        })
        .unwrap();
    press_enter(window, cx);
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.cloud_draft.as_ref().unwrap().error.is_some());
            assert_eq!(view.input.read(cx).content.as_ref(), "fix it");
        })
        .unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

#[gpui::test]
fn attachments_make_send_show_the_inline_error(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.attachments.push("/tmp/a.png".into());
            type_prompt(view, "fix it", cx);
        })
        .unwrap();
    press_enter(window, cx);
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            let error = view.cloud_draft.as_ref().unwrap().error.clone().unwrap();
            assert!(error.starts_with("Attachments cannot be queued"));
        })
        .unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

#[gpui::test]
fn the_local_target_still_sends_normally(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            assert!(!view.cloud_create_pending());
            assert!(view.cloud_send_tip(true).is_none());
            type_prompt(view, "hello", cx);
        })
        .unwrap();
    press_enter(window, cx);
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert!(!view.cloud_creating()))
        .unwrap();
    assert!(requests.lock().unwrap().is_empty());
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
fn the_panel_labels_are_the_dialogs_labels() {
    // One module of presets and labels serves the panel and the Cloud settings dialog.
    use crate::cloud_presets as presets;
    assert_eq!(presets::size_label(2, "4g"), "2 CPUs, 4 GB");
    assert_eq!(presets::runtime_label("1h"), "1 hour, then stop");
    assert_eq!(presets::runtime_label("4h"), "4 hours, then stop");
    assert_eq!(presets::idle_label(30), "After 30 minutes");
    assert_eq!(presets::idle_label(120), "After 2 hours");
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

/// Create requests sent by the first prompt on `window`.
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
            assert_eq!(view.follow_ups.state.messages.len(), 1);
            view.cloud_create_finished(id, Ok(true), cx);
            assert!(view.cloud_draft.is_none());
            // The new session holds the stored copy now; this draft keeps no queued one.
            assert!(view.follow_ups.state.messages.is_empty());
            assert_eq!(view.input.read(cx).content.as_ref(), "");
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
    let sandbox = monocode_core::cloud_sessions::created_note_for(
        &monocode_core::cloud_sessions::CloudRef::sandbox("sbx-2"),
        "x",
    );
    assert_eq!(created_codespace(&sandbox), Some("sbx-2"));
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
            assert_eq!(
                rows(view)[0].3.as_deref(),
                Some("Paid compute in your Docker account \u{b7} Set up first"),
                "no config yet"
            );
            // Saved behind the cache's back (Cloud settings): not seen until reopened.
            let config = DockerProjectConfig {
                repo_url: "https://github.com/octo/widgets.git".into(),
                ..Default::default()
            };
            docker_config::save(&locked(&view.store).unwrap(), PROJECT, &config).unwrap();
            assert_eq!(
                rows(view)[0].3.as_deref(),
                Some("Paid compute in your Docker account \u{b7} Set up first")
            );
            view.forget_docker_config();
            assert_ne!(
                rows(view)[0].3.as_deref(),
                Some("Paid compute in your Docker account \u{b7} Set up first")
            );
        })
        .unwrap();
}

// ------------------------------------------------ per-session overrides of the defaults

fn machine(name: &str, display: &str) -> codespaces::MachineType {
    codespaces::MachineType {
        name: name.into(),
        display_name: display.into(),
        operating_system: "linux".into(),
        storage_in_bytes: 32 << 30,
        memory_in_bytes: 8 << 30,
        cpus: 2,
    }
}

fn set_text(
    view: &Conversation,
    field: impl Fn(&CloudDraft) -> Entity<input::Input>,
    text: &str,
    cx: &mut Context<Conversation>,
) {
    let field = field(view.cloud_draft.as_ref().unwrap());
    field.update(cx, |input, cx| input.set_text(text.into(), cx));
}

#[gpui::test]
fn the_panel_starts_from_the_project_config(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            let config = DockerProjectConfig {
                repo_url: "https://github.com/octo/widgets.git".into(),
                base_ref: "develop".into(),
                cpus: 4,
                memory: "8g".into(),
                ttl: "4h".into(),
                claude_account: Some("work".into()),
                ..Default::default()
            };
            docker_config::save(&locked(&view.store).unwrap(), PROJECT, &config).unwrap();
            view.forget_docker_config();
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(
                (draft.cpus, draft.memory.as_str(), draft.ttl.as_str()),
                (4, "8g", "4h")
            );
            assert_eq!(draft.account.as_deref(), Some("work"));
            assert_eq!(draft.base.read(cx).content.as_ref(), "develop");
            assert_eq!(draft.base_text, "develop");
            // The menus check the default.
            let (_, compute) = view.cloud_field_menu(CloudField::Size);
            let checked: Vec<_> = compute
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Action(a) if a.checked => Some(a.label.to_string()),
                    _ => None,
                })
                .collect();
            assert_eq!(checked, ["4 CPUs, 8 GB"]);
        })
        .unwrap();
}

#[gpui::test]
fn overrides_change_the_sandbox_request_and_never_the_project_config(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            let saved = save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert!(view.cloud_menu_choice("cloud-set:size:4:8g", cx));
            assert!(view.cloud_menu_choice("cloud-set:runtime:1h", cx));
            assert!(view.cloud_menu_choice("cloud-set:claude:personal", cx));
            set_text(view, |d| d.base.clone(), "release/1.2", cx);
            set_text(view, |d| d.branch.clone(), "feature/x", cx);
            view.create_cloud(cx);
            // The saved default is untouched.
            let stored = docker_config::load(&locked(&view.store).unwrap(), PROJECT)
                .unwrap()
                .unwrap();
            assert_eq!(stored, saved);
            assert_eq!(stored.base_ref, "develop");
            assert_eq!((stored.cpus, stored.ttl.as_str()), (2, "2h"));
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let config = sent[0].config.as_ref().unwrap();
    assert_eq!(
        (config.cpus, config.memory.as_str(), config.ttl.as_str()),
        (4, "8g", "1h")
    );
    assert_eq!(config.base_ref, "release/1.2");
    assert_eq!(config.claude_account.as_deref(), Some("personal"));
    assert_eq!(config.repo_url, "https://github.com/octo/widgets.git");
    assert_eq!(sent[0].branch, "feature/x");
    assert_eq!(sent[0].machine, None);
}

/// The Size menu's `(id, checked)` rows.
fn size_rows(view: &Conversation) -> Vec<(String, bool)> {
    let (_, items) = view.cloud_field_menu(CloudField::Size);
    items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action(a) => Some((a.id.to_string(), a.checked)),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn a_size_below_the_devcontainer_warns_but_stays_selectable_and_is_not_raised(
    cx: &mut TestAppContext,
) {
    // This repository's devcontainer asks for 4 CPUs and 8 GB; the saved size is the default,
    // 2 CPUs / 4g. Like Cloud settings, the composer warns and changes nothing by itself.
    let repo = temp_repo("ref: refs/heads/main\n");
    let dir = std::path::Path::new(&repo).join(".devcontainer");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("devcontainer.json"),
        r#"{ "hostRequirements": { "cpus": 4, "memory": "8gb" } }"#,
    )
    .unwrap();
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            // Not raised: the draft keeps the saved size.
            assert_eq!((draft.cpus, draft.memory.as_str()), (2, "4g"));
            let warning = draft.size_warning().unwrap();
            assert_eq!(
                warning,
                "Below what the devcontainer asks for 4 CPUs and 8 GB. The environment may not build or run."
            );
            // Every size is still offered, the smaller ones too, and the saved one is checked.
            let rows = size_rows(view);
            assert_eq!(rows.len(), 5, "{rows:?}");
            assert_eq!(
                rows.iter()
                    .filter(|(_, checked)| *checked)
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>(),
                ["cloud-set:size:2:4g"]
            );
            assert!(rows.iter().any(|(id, _)| id == "cloud-set:size:1:2g"));
            // A smaller one can be chosen, and a size that meets the devcontainer ends the warning.
            assert!(view.cloud_menu_choice("cloud-set:size:1:2g", cx));
            assert!(view.cloud_draft.as_ref().unwrap().size_warning().is_some());
            assert!(view.cloud_menu_choice("cloud-set:size:4:8g", cx));
            assert_eq!(view.cloud_draft.as_ref().unwrap().size_warning(), None);
            assert!(view.cloud_menu_choice("cloud-set:size:2:4g", cx));
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    let config = sent[0].config.as_ref().unwrap();
    assert_eq!((config.cpus, config.memory.as_str()), (2, "4g"));
    drop(sent);
    // A project without a devcontainer has nothing to warn about.
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert_eq!(view.cloud_draft.as_ref().unwrap().size_warning(), None);
        })
        .unwrap();
}

#[gpui::test]
fn an_invalid_override_is_refused_inline_with_the_config_validation(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            set_text(view, |d| d.base.clone(), "bad branch name", cx);
            view.create_cloud(cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert!(!draft.creating);
            assert_eq!(
                draft.error.as_deref(),
                Some("The base branch contains invalid characters.")
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert!(requests.lock().unwrap().is_empty());
}

#[gpui::test]
fn picking_the_target_again_resets_the_overrides_to_the_project_config(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.cloud_menu_choice("cloud-set:runtime:4h", cx);
            set_text(view, |d| d.base.clone(), "other", cx);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.ttl, "2h");
            assert_eq!(draft.base.read(cx).content.as_ref(), "develop");
            // Nothing changes while a create runs.
            view.create_cloud(cx);
            view.cloud_menu_choice("cloud-set:runtime:1h", cx);
            assert_eq!(view.cloud_draft.as_ref().unwrap().ttl, "2h");
        })
        .unwrap();
}

#[gpui::test]
fn machines_load_off_the_spot_and_the_pick_reaches_the_request(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    let listed = Arc::new(Mutex::new(Vec::new()));
    let sink = listed.clone();
    window
        .update(cx, |view, _, cx| {
            view.set_machine_loader(Arc::new(move |project, _| {
                sink.lock().unwrap().push(project);
                Ok(vec![
                    machine("basicLinux32gb", "2 cores, 8 GB RAM"),
                    machine("standardLinux32gb", "4 cores, 16 GB RAM"),
                ])
            }));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            // The listing runs on the executor, not inside the click; the default stays.
            assert!(listed.lock().unwrap().is_empty());
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.machines, Machines::Loading);
            assert_eq!(view.cloud_field_value(CloudField::Machine), "Loading machines...");
            let (_, items) = view.cloud_field_menu(CloudField::Machine);
            assert!(matches!(&items[0], MenuItem::Action(a) if a.disabled && a.label.contains("Loading")));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(*listed.lock().unwrap(), std::slice::from_ref(&repo));
    window
        .update(cx, |view, _, cx| {
            assert!(matches!(
                view.cloud_draft.as_ref().unwrap().machines,
                Machines::Loaded(_)
            ));
            // Still the default until a pick.
            assert_eq!(
                view.cloud_field_value(CloudField::Machine),
                "2 cores, 8 GB RAM"
            );
            let (_, items) = view.cloud_field_menu(CloudField::Machine);
            assert_eq!(items.len(), 2);
            assert!(view.cloud_menu_choice("cloud-set:machine:standardLinux32gb", cx));
            assert_eq!(
                view.cloud_field_value(CloudField::Machine),
                "4 cores, 16 GB RAM"
            );
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].machine.as_deref(), Some("standardLinux32gb"));
    assert_eq!(sent[0].branch, "main");
}

#[gpui::test]
fn a_failed_machine_listing_keeps_the_default_machine(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_machine_loader(Arc::new(|_, _| Err("gh is not installed".into())));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.cloud_draft.as_ref().unwrap().machines,
                Machines::Failed
            );
            assert_eq!(
                view.cloud_field_value(CloudField::Machine),
                codespaces::DEFAULT_MACHINE
            );
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    // No pick: the request keeps the default machine.
    assert_eq!(requests.lock().unwrap()[0].machine, None);
}

/// A machine type with a size, as `gh api repos/o/r/codespaces/machines` reports it.
fn sized(name: &str, cpus: u32, memory_gb: u64) -> codespaces::MachineType {
    codespaces::MachineType {
        name: name.into(),
        display_name: format!("{cpus} cores, {memory_gb} GB RAM"),
        operating_system: "linux".into(),
        storage_in_bytes: 32 << 30,
        memory_in_bytes: memory_gb << 30,
        cpus,
    }
}

#[gpui::test]
fn only_allowed_machines_are_offered_and_the_smallest_is_the_default(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let sink = asked.clone();
    window
        .update(cx, |view, _, cx| {
            // A devcontainer asking for 4 cores: GitHub leaves basicLinux32gb out, in any order.
            view.set_machine_loader(Arc::new(move |_, branch| {
                sink.lock().unwrap().push(branch);
                Ok(vec![
                    sized("premiumLinux", 8, 32),
                    sized("standardLinux32gb", 4, 16),
                    sized("largePremiumLinux", 16, 64),
                ])
            }));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    // Asked once, for the branch the codespace would be created on.
    assert_eq!(*asked.lock().unwrap(), vec![Some("main".to_string())]);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.cloud_field_value(CloudField::Machine),
                "4 cores, 16 GB RAM"
            );
            let (_, items) = view.cloud_field_menu(CloudField::Machine);
            let offered: Vec<(String, bool)> = items
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Action(a) => Some((a.id.to_string(), a.checked)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                offered,
                vec![
                    ("cloud-set:machine:premiumLinux".to_string(), false),
                    ("cloud-set:machine:standardLinux32gb".to_string(), true),
                    ("cloud-set:machine:largePremiumLinux".to_string(), false),
                ]
            );
            assert!(
                !offered
                    .iter()
                    .any(|(id, _)| id.contains(codespaces::DEFAULT_MACHINE))
            );
            assert_eq!(view.cloud_draft.as_ref().unwrap().machine_note, None);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    // No pick: the start takes the smallest machine allowed on the branch it creates on (the
    // same one here), never the static basicLinux32gb.
    assert_eq!(requests.lock().unwrap()[0].machine, None);
}

#[gpui::test]
fn a_pick_another_branch_does_not_allow_falls_back_with_a_note(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let sink = asked.clone();
    window
        .update(cx, |view, _, cx| {
            view.set_machine_loader(Arc::new(move |_, branch: Option<String>| {
                sink.lock().unwrap().push(branch.clone());
                Ok(match branch.as_deref() {
                    // This branch's devcontainer needs 4 cores.
                    Some("big") => vec![
                        sized("standardLinux32gb", 4, 16),
                        sized("premiumLinux", 8, 32),
                    ],
                    _ => vec![
                        sized("basicLinux32gb", 2, 8),
                        sized("standardLinux32gb", 4, 16),
                        sized("premiumLinux", 8, 32),
                    ],
                })
            }));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.cloud_field_value(CloudField::Machine),
                "2 cores, 8 GB RAM"
            );
            // A pick the next branch still allows is kept.
            assert!(view.cloud_menu_choice("cloud-set:machine:premiumLinux", cx));
            view.cloud_ref_picked(CloudRefField::Base, Some("big".into()), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.machine.as_deref(), Some("premiumLinux"));
            assert_eq!(draft.machine_note, None);
            // Back to main, pick the 2-core machine, then a branch that rules it out.
            view.cloud_ref_picked(CloudRefField::Base, Some("main".into()), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.cloud_menu_choice("cloud-set:machine:basicLinux32gb", cx));
            view.cloud_ref_picked(CloudRefField::Base, Some("big".into()), cx);
            // The same branch again asks nothing more.
            view.cloud_ref_picked(CloudRefField::Base, Some("big".into()), cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *asked.lock().unwrap(),
        vec![
            Some("main".to_string()),
            Some("big".to_string()),
            Some("main".to_string()),
            Some("big".to_string()),
        ]
    );
    window
        .update(cx, |view, _, cx| {
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.machine, None);
            let note = draft.machine_note.clone().unwrap();
            assert!(note.contains("basicLinux32gb"), "{note}");
            assert!(note.contains("4 cores, 16 GB RAM"), "{note}");
            assert_eq!(
                view.cloud_field_value(CloudField::Machine),
                "4 cores, 16 GB RAM"
            );
            // Picking a machine again clears the note.
            assert!(view.cloud_menu_choice("cloud-set:machine:premiumLinux", cx));
            assert_eq!(view.cloud_draft.as_ref().unwrap().machine_note, None);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent[0].machine.as_deref(), Some("premiumLinux"));
    assert_eq!(sent[0].branch, "big");
}

#[gpui::test]
fn a_late_machine_listing_for_a_replaced_draft_is_ignored(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            view.set_machine_loader(Arc::new(|_, _| Ok(vec![machine("a", "A")])));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            view.clear_cloud_target(cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(matches!(
                view.cloud_draft.as_ref().unwrap().machines,
                Machines::Loaded(_)
            ));
        })
        .unwrap();
}

#[gpui::test]
fn the_panel_has_a_row_per_setting_and_a_collapse_chevron(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let draw = |visual: &mut gpui::VisualTestContext| {
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    draw(visual);
    for id in [
        "cloud-target-panel",
        "cloud-target-header",
        "cloud-row-size",
        "cloud-row-runtime",
        "cloud-row-base",
        "cloud-row-branch",
    ] {
        assert!(visual.debug_bounds(id).is_some(), "{id}");
    }
    // Collapsed: only the header stays.
    let header = visual.debug_bounds("cloud-target-header").unwrap();
    visual.simulate_click(header.center(), Default::default());
    draw(visual);
    assert!(visual.debug_bounds("cloud-target-header").is_some());
    assert!(visual.debug_bounds("cloud-row-size").is_none());
    // A codespace panel has its own rows.
    window
        .update(visual, |view, _, cx| {
            view.clear_cloud_target(cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-row-machine").is_some());
    assert!(visual.debug_bounds("cloud-row-branch").is_some());
    assert!(visual.debug_bounds("cloud-row-size").is_none());
    // A settings row opens its menu.
    let machine = visual.debug_bounds("cloud-row-machine").unwrap();
    visual.simulate_click(machine.center(), Default::default());
    draw(visual);
    window
        .update(visual, |view, _, _| {
            assert!(view.menu_is(ComposerMenu::CloudField(CloudField::Machine)))
        })
        .unwrap();
}

#[gpui::test]
fn the_error_from_a_send_stays_visible_while_the_panel_is_collapsed(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let draw = |visual: &mut gpui::VisualTestContext| {
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            set_text(view, |d| d.branch.clone(), "", cx);
            type_prompt(view, "fix it", cx);
        })
        .unwrap();
    draw(visual);
    let header = visual.debug_bounds("cloud-target-header").unwrap();
    visual.simulate_click(header.center(), Default::default());
    draw(visual);
    assert!(visual.debug_bounds("cloud-row-size").is_none());
    let send = visual.debug_bounds("send-or-stop").unwrap();
    visual.simulate_click(send.center(), Default::default());
    draw(visual);
    assert!(visual.debug_bounds("cloud-target-error").is_some());
    assert!(requests.lock().unwrap().is_empty());
}

#[gpui::test]
fn the_branch_fields_are_read_only_while_creating(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.create_cloud(cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            let id = draft.request;
            assert!(draft.branch.read(cx).is_read_only());
            assert!(draft.base.read(cx).is_read_only());
            view.cloud_create_finished(id, Err("nope".into()), cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert!(!draft.branch.read(cx).is_read_only());
            assert!(!draft.base.read(cx).is_read_only());
        })
        .unwrap();
}

#[gpui::test]
fn picking_the_codespace_again_resets_the_machine_branch_and_listing(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let listed = Arc::new(Mutex::new(0));
    let count = listed.clone();
    window
        .update(cx, |view, _, cx| {
            view.set_machine_loader(Arc::new(move |_, _| {
                *count.lock().unwrap() += 1;
                Ok(vec![machine("basicLinux32gb", "Basic")])
            }));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.cloud_menu_choice("cloud-set:machine:basicLinux32gb", cx);
            set_text(view, |d| d.branch.clone(), "other", cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.machine, None);
            assert_eq!(draft.machines, Machines::Loading);
            assert_eq!(draft.branch.read(cx).content.as_ref(), "main");
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(*listed.lock().unwrap(), 2);
    window
        .update(cx, |view, _, _| {
            assert!(matches!(
                view.cloud_draft.as_ref().unwrap().machines,
                Machines::Loaded(_)
            ));
        })
        .unwrap();
}

/// Draws the window once the pending work is done.
fn draw(visual: &mut gpui::VisualTestContext) {
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn right_edge(bounds: gpui::Bounds<gpui::Pixels>) -> f32 {
    f32::from(bounds.right())
}

#[gpui::test]
fn each_setting_menu_anchors_to_the_value_of_the_clicked_row(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    draw(visual);
    let mut tops = Vec::new();
    for (field, row_id, anchor_id) in [
        (CloudField::Size, "cloud-row-size", "cloud-row-size-anchor"),
        (
            CloudField::Runtime,
            "cloud-row-runtime",
            "cloud-row-runtime-anchor",
        ),
    ] {
        let row = visual.debug_bounds(row_id).unwrap();
        visual.simulate_click(row.center(), Default::default());
        draw(visual);
        let value = visual.debug_bounds(anchor_id).unwrap();
        let slot = window
            .update(visual, |view, _, _| {
                assert!(view.menu_is(ComposerMenu::CloudField(field)));
                view.anchor(ComposerMenu::CloudField(field)).get().unwrap()
            })
            .unwrap();
        // The menu is anchored to this row's value, not to the panel or the whole row.
        assert_eq!(slot, value, "{row_id}");
        assert!(row.contains(&slot.origin) && slot.size.width < row.size.width / 2.);
        assert!(right_edge(slot) <= right_edge(row) + 0.5);
        assert!(slot.left() > row.left() + row.size.width / 2.);
        tops.push(slot.top());
        // An outside press only dismisses an open menu, so close it before the next row.
        window
            .update(visual, |view, window, cx| view.close_menu(window, cx))
            .unwrap();
        draw(visual);
    }
    // Each row has its own slot.
    assert_ne!(tops[0], tops[1]);
}

#[gpui::test]
fn a_created_codespace_disables_send(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            view.create_cloud(cx);
            let id = view.cloud_draft.as_ref().unwrap().request;
            view.cloud_create_finished(
                id,
                Err("Failed (codespace abc was created and kept)".into()),
                cx,
            );
            assert!(view.cloud_draft.as_ref().unwrap().created.is_some());
        })
        .unwrap();
    window
        .update(visual, |view, _, cx| {
            type_prompt(view, "again", cx);
            let (tip, usable) = view.cloud_send_tip(true).unwrap();
            assert!(tip.starts_with("Already created"));
            assert!(!usable);
        })
        .unwrap();
    draw(visual);
    let send = visual.debug_bounds("send-or-stop").unwrap();
    visual.simulate_click(send.center(), Default::default());
    press_enter(window, visual);
    visual.run_until_parked();
    draw(visual);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

/// A branch source with fixed answers: the local tracking branches (`None`: no matching remote)
/// and the `ls-remote` result. Records the thread the local read ran on.
fn source(
    local: Option<&[&str]>,
    heads: Result<&[&str], &str>,
    thread: Arc<Mutex<Option<std::thread::ThreadId>>>,
) -> BranchSource {
    let local = local.map(|names| RemoteBranches {
        remote: "origin".into(),
        url: "https://github.com/octo/widgets.git".into(),
        branches: names.iter().map(|n| n.to_string()).collect(),
    });
    let heads = heads
        .map(|names| names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
        .map_err(str::to_owned);
    BranchSource {
        local: Arc::new(move |_, _| {
            *thread.lock().unwrap() = Some(std::thread::current().id());
            local.clone()
        }),
        heads: Arc::new(move |_, _| heads.clone()),
    }
}

fn names(view: &Conversation) -> Vec<String> {
    match &view.cloud_draft.as_ref().unwrap().branches {
        BranchList::Ready { branches, .. } => branches.clone(),
        other => panic!("{other:?}"),
    }
}

#[gpui::test]
fn the_branch_list_comes_from_a_source_off_the_click(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let thread = Arc::new(Mutex::new(None));
    let seen = thread.clone();
    window
        .update(cx, |view, _, cx| {
            view.set_branch_source(source(Some(&["main", "dev"]), Err("offline"), seen));
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            // Nothing was read inside the click: the free field stays until the answer.
            assert!(thread.lock().unwrap().is_none());
            assert_eq!(
                view.cloud_draft.as_ref().unwrap().branches,
                BranchList::Loading
            );
            assert!(!view.cloud_picks(CloudRefField::Base));
        })
        .unwrap();
    cx.run_until_parked();
    assert!(thread.lock().unwrap().is_some(), "the source was asked");
    window
        .update(cx, |view, _, _| {
            assert_eq!(names(view), ["main", "dev"]);
            assert!(view.cloud_picks(CloudRefField::Base));
        })
        .unwrap();
}

#[gpui::test]
fn a_repository_without_a_matching_remote_keeps_the_text_field(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            view.set_branch_source(source(None, Ok(&["main"]), Default::default()));
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    draw(visual);
    window
        .update(visual, |view, _, _| {
            assert_eq!(
                view.cloud_draft.as_ref().unwrap().branches,
                BranchList::Unmatched
            );
            assert!(!view.cloud_picks(CloudRefField::Base));
        })
        .unwrap();
    // The row is still there, with its text field, and the typed value reaches the request.
    assert!(visual.debug_bounds("cloud-row-base").is_some());
    assert!(visual.debug_bounds("cloud-row-base-anchor").is_none());
}

#[gpui::test]
fn picking_a_base_branch_sets_the_override_and_reaches_the_request(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            view.set_branch_source(source(
                Some(&["main", "release/1", "bad name"]),
                Err("offline"),
                Default::default(),
            ));
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    draw(visual);
    let row = visual.debug_bounds("cloud-row-base").unwrap();
    visual.simulate_click(row.center(), Default::default());
    draw(visual);
    let anchor = visual.debug_bounds("cloud-row-base-anchor").unwrap();
    let picker = window
        .update(visual, |view, _, cx| {
            let open = view
                .cloud_picker
                .as_ref()
                .expect("the branch picker is open");
            assert_eq!(open.field, CloudRefField::Base);
            // The picker is anchored to this row's value, like the setting menus.
            assert_eq!(
                view.cloud_ref_anchors[CloudRefField::Base.index()].get(),
                Some(anchor)
            );
            // The saved config's base shows as the current one.
            assert_eq!(view.cloud_ref_value(CloudRefField::Base, cx), "develop");
            open.picker.clone()
        })
        .unwrap();
    // The default entry first, then the branches the validators accept.
    window
        .update(visual, |_, _, cx| {
            picker.update(cx, |_, cx| {
                cx.emit(GitPicked::Ref(Some("release/1".into())))
            });
        })
        .unwrap();
    draw(visual);
    window
        .update(visual, |view, _, cx| {
            assert!(view.cloud_picker.is_none());
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.base_text, "release/1");
            assert_eq!(draft.base.read(cx).content.as_ref(), "release/1");
            // An invalid name never reaches the draft.
            view.cloud_ref_picked(CloudRefField::Base, Some("bad name".into()), cx);
            assert_eq!(view.cloud_draft.as_ref().unwrap().base_text, "release/1");
            view.create_cloud(cx);
        })
        .unwrap();
    draw(visual);
    let sent = requests.lock().unwrap().clone();
    assert_eq!(sent[0].config.as_ref().unwrap().base_ref, "release/1");
    // Nothing was written back to the project config.
    window
        .update(visual, |view, _, _| {
            view.forget_docker_config();
            assert_eq!(view.docker_config().unwrap().base_ref, "develop");
        })
        .unwrap();
}

#[gpui::test]
fn the_default_branch_entry_clears_the_base_ref(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_branch_source(source(Some(&["main"]), Err("offline"), Default::default()));
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            view.toggle_cloud_ref_picker(CloudRefField::Base, window, cx);
            let picker = view.cloud_picker.as_ref().unwrap().picker.clone();
            picker.update(cx, |_, cx| cx.emit(GitPicked::Ref(None)));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.cloud_ref_value(CloudRefField::Base, cx),
                "Default branch"
            );
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        requests.lock().unwrap()[0]
            .config
            .as_ref()
            .unwrap()
            .base_ref,
        ""
    );
}

#[gpui::test]
fn a_codespace_branch_is_picked_from_origin_and_reaches_the_request(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_branch_source(source(
                Some(&["main", "topic"]),
                Err("offline"),
                Default::default(),
            ));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert!(view.cloud_picks(CloudRefField::Base));
            assert_eq!(view.cloud_ref_value(CloudRefField::Base, cx), "main");
            view.toggle_cloud_ref_picker(CloudRefField::Base, window, cx);
            let picker = view.cloud_picker.as_ref().unwrap().picker.clone();
            picker.update(cx, |_, cx| cx.emit(GitPicked::Ref(Some("topic".into()))));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| view.create_cloud(cx))
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent[0].branch, "topic");
    // The working branch followed the base, so no extra branch is asked for.
    assert_eq!(sent[0].work_branch, None);
}

#[gpui::test]
fn a_failed_ls_remote_keeps_the_local_list_and_a_good_one_refreshes_it(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            view.set_branch_source(source(
                Some(&["main", "gone"]),
                Err("auth failed"),
                Default::default(),
            ));
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(names(view), ["main", "gone"]);
            assert!(view.cloud_draft.as_ref().unwrap().error.is_none());
        })
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.set_branch_source(source(
                Some(&["main", "gone"]),
                Ok(&["zeta", "main", "alpha"]),
                Default::default(),
            ));
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            // Branches the remote still has keep the local order; new ones follow, sorted; the
            // stale tracking branch is dropped.
            assert_eq!(names(view), ["main", "alpha", "zeta"]);
        })
        .unwrap();
}

#[test]
fn the_matching_remote_is_chosen_by_repository_url() {
    let remotes = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, u)| (n.to_string(), u.to_string()))
            .collect()
    };
    let repo = RepoMatch::Url("https://github.com/Octo/Widgets.git".into());
    // ssh, scp-style and https forms of the same repository all match; origin wins a tie.
    for url in [
        "git@github.com:octo/widgets.git",
        "ssh://git@github.com/octo/widgets",
        "https://github.com/octo/widgets/",
        "https://user@github.com/octo/widgets.git",
    ] {
        assert_eq!(
            pick_remote(
                &remotes(&[("fork", "git@github.com:me/widgets.git"), ("up", url)]),
                &repo
            ),
            Some("up".into()),
            "{url}"
        );
    }
    assert_eq!(
        pick_remote(
            &remotes(&[
                ("up", "https://github.com/octo/widgets.git"),
                ("origin", "git@github.com:octo/widgets.git"),
            ]),
            &repo
        ),
        Some("origin".into())
    );
    // Another repository, another host, or no remote at all: no match.
    assert_eq!(
        pick_remote(
            &remotes(&[("origin", "git@github.com:octo/other.git")]),
            &repo
        ),
        None
    );
    assert_eq!(
        pick_remote(
            &remotes(&[("origin", "git@gitlab.com:octo/widgets.git")]),
            &repo
        ),
        None
    );
    assert_eq!(pick_remote(&[], &repo), None);
    // A codespace always means origin, whatever its URL.
    assert_eq!(
        pick_remote(
            &remotes(&[("origin", "git@github.com:me/x.git")]),
            &RepoMatch::Origin
        ),
        Some("origin".into())
    );
    assert_eq!(
        pick_remote(
            &remotes(&[("up", "git@github.com:me/x.git")]),
            &RepoMatch::Origin
        ),
        None
    );
}

#[test]
fn the_real_local_read_uses_the_tracking_branches_of_the_matching_remote() {
    let dir = std::path::PathBuf::from(temp_repo("ref: refs/heads/main\n"));
    std::fs::remove_dir_all(dir.join(".git")).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "one"]);
    git(&["remote", "add", "origin", "git@github.com:me/widgets.git"]);
    git(&[
        "remote",
        "add",
        "upstream",
        "https://github.com/octo/widgets.git",
    ]);
    git(&["update-ref", "refs/remotes/upstream/main", "HEAD"]);
    git(&["update-ref", "refs/remotes/upstream/dev", "HEAD"]);
    git(&[
        "symbolic-ref",
        "refs/remotes/upstream/HEAD",
        "refs/remotes/upstream/main",
    ]);
    git(&["update-ref", "refs/remotes/origin/mine", "HEAD"]);
    let project = dir.to_string_lossy().into_owned();
    let source = BranchSource::default();
    let docker = (source.local)(
        &project,
        &RepoMatch::Url("https://github.com/octo/widgets".into()),
    )
    .expect("upstream matches");
    assert_eq!(docker.remote, "upstream");
    let mut branches = docker.branches.clone();
    branches.sort();
    assert_eq!(
        branches,
        ["dev", "main"],
        "the remote HEAD alias is not a branch"
    );
    let codespace = (source.local)(&project, &RepoMatch::Origin).unwrap();
    assert_eq!(codespace.branches, ["mine"]);
    assert!(
        (source.local)(
            &project,
            &RepoMatch::Url("https://github.com/octo/other".into())
        )
        .is_none()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ls_remote_output_is_parsed_and_merged() {
    assert_eq!(
        parse_heads("abc\trefs/heads/main\ndef\trefs/heads/feat/x\nghi\trefs/tags/v1\n"),
        ["main", "feat/x"]
    );
    assert_eq!(
        merge_heads(&["a".into(), "b".into()], vec!["c".into(), "b".into()]),
        ["b", "c"]
    );
}

#[test]
fn remotes_are_read_nul_separated_and_option_like_names_are_refused() {
    let config = "remote.my fork.url\nhttps://github.com/octo/widgets.git\0remote.-x.url\nhttps://github.com/octo/widgets\0";
    let remotes = parse_remotes(config);
    assert_eq!(remotes[0].0, "my fork");
    let repo = RepoMatch::Url("https://github.com/octo/widgets".into());
    // The option-like name matches the URL too but never becomes a git argument.
    assert_eq!(pick_remote(&remotes, &repo), Some("my fork".into()));
    assert_eq!(pick_remote(&remotes[1..], &repo), None);
    assert_eq!(
        pick_remote(&[("-origin".into(), "x".into())], &RepoMatch::Origin),
        None
    );
}

#[cfg(unix)]
#[test]
fn a_timed_out_ls_remote_kills_the_whole_process_group() {
    let dir = std::path::PathBuf::from(temp_repo("ref: refs/heads/main\n"));
    let pidfile = dir.join("child.pid");
    let mut command = std::process::Command::new("sh");
    command.arg("-c").arg(format!(
        "sleep 60 & echo $! > '{}'; wait",
        pidfile.display()
    ));
    let result = run_with_timeout(command, std::time::Duration::from_millis(500));
    assert_eq!(result.unwrap_err(), "git ls-remote timed out");
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The grandchild (the stand-in for ssh) dies with the group.
    let mut alive = true;
    for _ in 0..50 {
        // SAFETY: signal 0 only checks that the process exists.
        alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        !alive,
        "the child of the timed out command is still running"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui::test]
fn a_late_branch_listing_for_a_replaced_draft_is_ignored(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.set_branch_source(source(
                Some(&["old"]),
                Ok(&["old", "stale"]),
                Default::default(),
            ));
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            // Replaced before the first listing answers.
            view.clear_cloud_target(cx);
            view.set_branch_source(source(Some(&["new"]), Err("offline"), Default::default()));
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert_eq!(names(view), ["new"]))
        .unwrap();
}

#[gpui::test]
fn a_stale_listing_does_not_touch_an_open_picker_or_the_new_draft(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.set_branch_source(source(Some(&["main"]), Err("offline"), Default::default()));
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            let stale = view.cloud_draft.as_ref().unwrap().branches_request;
            view.toggle_cloud_ref_picker(CloudRefField::Base, window, cx);
            let picker = view.cloud_picker.as_ref().unwrap().picker.clone();
            // Answers carrying another request id change neither the draft nor the picker.
            view.apply_refreshed_branches(stale + 100, "origin".into(), vec!["late".into()], cx);
            view.apply_local_branches(stale + 100, None, cx);
            assert_eq!(names(view), ["main"]);
            assert_eq!(picker.read(cx).offered(), ["main"]);
            // The current one still applies to both.
            view.apply_refreshed_branches(
                stale,
                "origin".into(),
                vec!["main".into(), "fresh".into()],
                cx,
            );
            assert_eq!(picker.read(cx).offered(), ["main", "fresh"]);
            // A replaced draft drops the picker, and the old request id is then stale.
            view.clear_cloud_target(cx);
            assert!(view.cloud_picker.is_none());
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.apply_refreshed_branches(stale, "origin".into(), vec!["ghost".into()], cx);
            assert_eq!(
                view.cloud_draft.as_ref().unwrap().branches,
                BranchList::Loading
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_codespace_branch_starting_with_a_dash_is_refused(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            view.set_branch_source(source(Some(&["main"]), Err("offline"), Default::default()));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.cloud_ref_picked(CloudRefField::Base, Some("--upload-pack=x".into()), cx);
            assert_eq!(view.cloud_ref_value(CloudRefField::Base, cx), "main");
            view.cloud_ref_picked(CloudRefField::Base, Some("v1.2.3".into()), cx);
            assert_eq!(view.cloud_ref_value(CloudRefField::Base, cx), "v1.2.3");
        })
        .unwrap();
}

#[gpui::test]
fn sends_that_are_not_the_users_never_create_a_cloud_resource(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            // The app API (`sessions.send`), Monos and mobile all end in `send`.
            view.install_transfer(Default::default(), Some("from the API".into()), cx);
            type_prompt(view, "from a Mono", cx);
            view.send(cx);
            assert!(!view.cloud_creating());
            assert_eq!(view.status, "Choose Send to create the Docker sandbox");
            assert_eq!(view.input.read(cx).content.as_ref(), "from a Mono");
        })
        .unwrap();
    cx.run_until_parked();
    assert!(requests.lock().unwrap().is_empty());
}

#[gpui::test]
fn a_create_that_ends_without_taking_the_prompt_puts_it_back_in_the_composer(
    cx: &mut TestAppContext,
) {
    for result in [Err("boom".to_owned()), Ok(false)] {
        let window = conversation(cx, PROJECT, false);
        let requests = watch(window, cx);
        window
            .update(cx, |view, _, cx| {
                save_config(view);
                view.cloud_menu_choice("workspace:cloud-docker", cx);
                type_prompt(view, "keep me", cx);
                view.create_cloud(cx);
                assert_eq!(view.follow_ups.state.messages.len(), 1);
                assert_eq!(view.input.read(cx).content.as_ref(), "");
            })
            .unwrap();
        cx.run_until_parked();
        let id = requests.lock().unwrap()[0].request_id;
        window
            .update(cx, |view, _, cx| {
                view.cloud_create_finished(id, result.clone(), cx);
                assert!(view.follow_ups.state.messages.is_empty());
                assert!(view.follow_ups.sending.is_none());
                assert_eq!(view.input.read(cx).content.as_ref(), "keep me");
            })
            .unwrap();
    }
}

#[gpui::test]
fn the_prompt_waiting_for_a_create_shows_in_the_queue_shelf_not_the_composer(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            type_prompt(view, "fix it", cx);
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("message-queue-card").is_none());
    window
        .update(visual, |view, _, cx| user_send(view, cx))
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("message-queue-card").is_some());
}

#[gpui::test]
fn the_prompt_held_for_a_create_is_never_persisted_with_the_local_draft(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            type_prompt(view, "fix it", cx);
            view.create_cloud(cx);
            assert_eq!(view.follow_ups.state.messages.len(), 1);
            // Another save while the create runs must not store the held prompt.
            view.save_follow_ups(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            // Still shown in the queue shelf after the save came back.
            assert_eq!(view.follow_ups.state.messages.len(), 1);
            let id = view.summary.id;
            let stored = view.store.lock().unwrap().follow_ups(id).unwrap().messages;
            assert!(stored.is_empty(), "{stored:?}");
        })
        .unwrap();
}

#[gpui::test]
fn plan_mode_stays_on_with_a_note_after_a_create(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            type_prompt(view, "fix it", cx);
            view.settings.plan = true;
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let id = requests.lock().unwrap()[0].request_id;
    window
        .update(cx, |view, _, cx| {
            view.cloud_create_finished(id, Ok(true), cx);
            assert!(view.settings.plan);
            assert!(
                view.status.contains("stay in the composer"),
                "{}",
                view.status
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_save_that_returns_after_the_create_finished_does_not_requeue_the_prompt(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            type_prompt(view, "fix it", cx);
            view.create_cloud(cx);
            view.save_follow_ups(cx);
        })
        .unwrap();
    let id = requests.lock().unwrap()[0].request_id;
    // The create finishes before the save comes back.
    window
        .update(cx, |view, _, cx| {
            view.cloud_create_finished(id, Ok(true), cx)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(view.follow_ups.state.messages.is_empty());
            assert!(view.launched.is_empty());
        })
        .unwrap();
}

fn kit_globals(cx: &mut TestAppContext, enabled: bool) {
    cx.update(|cx| {
        cx.set_global(crate::cloud_kit_ui::KitScanner(
            crate::cloud_kit_ui::fixture::scan,
        ));
        let mut prefs = crate::settings::Preferences::default();
        if enabled {
            prefs.cloud_kit.acknowledge_notice();
        }
        cx.set_global(prefs);
    });
}

fn choose_docker(window: WindowHandle<Conversation>, visual: &mut gpui::VisualTestContext) {
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
}

#[gpui::test]
fn the_panel_shows_the_kit_summary_and_a_switch_for_this_start(cx: &mut TestAppContext) {
    use crate::cloud_kit_ui::Summary;
    use monocode_core::cloud_sessions::kit_prefs::KitSummary;
    kit_globals(cx, true);
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    choose_docker(window, visual);
    assert!(visual.debug_bounds("cloud-row-kit").is_some());
    assert!(visual.debug_bounds("cloud-kit-settings").is_some());
    window
        .update(visual, |view, _, _| {
            let draft = view.cloud_draft.as_ref().unwrap();
            assert!(draft.kit, "a start begins with the user's standing choice");
            assert_eq!(
                draft.kit_summary,
                Summary::Ready(KitSummary {
                    skills: 2,
                    mcp_servers: 3,
                    plugins: 1,
                    ..Default::default()
                })
            );
        })
        .unwrap();

    // The switch turns the kit off for this start only.
    let switch = visual.debug_bounds("cloud-kit-switch").unwrap();
    visual.simulate_click(switch.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            assert!(!view.cloud_draft.as_ref().unwrap().kit);
            assert!(
                crate::cloud_kit_ui::kit_prefs(cx).enabled,
                "Settings is untouched"
            );
        })
        .unwrap();
}

#[gpui::test]
fn the_settings_link_asks_the_shell_to_open_cloud_settings(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    kit_globals(cx, true);
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let opened = Arc::new(AtomicUsize::new(0));
    let sink = opened.clone();
    window
        .update(visual, |_, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, _: &OpenCloudSettings, _| {
                sink.fetch_add(1, Ordering::SeqCst);
            })
            .detach();
        })
        .unwrap();
    choose_docker(window, visual);
    let link = visual.debug_bounds("cloud-kit-settings").unwrap();
    visual.simulate_click(link.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(opened.load(Ordering::SeqCst), 1);
}

#[gpui::test]
fn the_kit_row_is_locked_while_the_kit_is_off(cx: &mut TestAppContext) {
    // The kit is off in Settings: the row says so and the switch does nothing.
    kit_globals(cx, false);
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    choose_docker(window, visual);
    assert!(visual.debug_bounds("cloud-row-kit").is_some());
    let switch = visual.debug_bounds("cloud-kit-switch").unwrap();
    visual.simulate_click(switch.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(!view.cloud_draft.as_ref().unwrap().kit);
        })
        .unwrap();
}

fn set_kit_prefs(cx: &mut TestAppContext, change: impl FnOnce(&mut crate::settings::Preferences)) {
    cx.update(|cx| {
        let mut prefs = cx.global::<crate::settings::Preferences>().clone();
        change(&mut prefs);
        cx.set_global(prefs);
    });
}

#[gpui::test]
fn create_never_sends_the_kit_after_settings_turned_it_off(cx: &mut TestAppContext) {
    kit_globals(cx, true);
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert!(view.cloud_draft.as_ref().unwrap().kit);
        })
        .unwrap();
    // Settings turns the kit off while the panel is open.
    set_kit_prefs(cx, |prefs| prefs.cloud_kit.disable());
    window
        .update(cx, |view, _, cx| view.create_cloud(cx))
        .unwrap();
    cx.run_until_parked();
    assert!(
        !requests.lock().unwrap()[0].kit,
        "Settings wins over the panel"
    );

    // Control: with the kit still on the request carries it.
    kit_globals(cx, true);
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(requests.lock().unwrap()[0].kit);
}

#[gpui::test]
fn a_sandbox_panel_counts_docker_servers_as_unavailable_and_a_codespace_panel_does_not(
    cx: &mut TestAppContext,
) {
    use crate::cloud_kit_ui::Summary;
    kit_globals(cx, true);
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    choose_docker(window, visual);
    let servers = |view: &Conversation| match view.cloud_draft.as_ref().unwrap().kit_summary {
        Summary::Ready(counts) => counts.mcp_servers,
        other => panic!("{other:?}"),
    };
    window
        .update(visual, |view, _, _| assert_eq!(servers(view), 3))
        .unwrap();
    window
        .update(visual, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            assert!(view.cloud_menu_choice("workspace:cloud-codespace", cx));
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| assert_eq!(servers(view), 4))
        .unwrap();
}

#[gpui::test]
fn the_summary_follows_settings_changes_while_the_panel_is_open(cx: &mut TestAppContext) {
    use crate::cloud_kit_ui::Summary;
    // The row counts as soon as the panel opens.
    kit_globals(cx, true);
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    choose_docker(window, visual);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("cloud-row-kit").is_some());
    window
        .update(visual, |view, _, _| {
            assert!(matches!(
                view.cloud_draft.as_ref().unwrap().kit_summary,
                Summary::Ready(counts) if counts.skills == 2
            ));
        })
        .unwrap();

    // Unchecking a skill in Settings changes the count.
    set_kit_prefs(cx, |prefs| {
        let inventory = crate::cloud_kit_ui::fixture::inventory();
        prefs
            .cloud_kit
            .set_selected(&inventory, "skill:user:review", false);
    });
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(matches!(
                view.cloud_draft.as_ref().unwrap().kit_summary,
                Summary::Ready(counts) if counts.skills == 1
            ));
        })
        .unwrap();
}

#[gpui::test]
fn a_cloud_session_shows_its_backend_and_workspace_state_in_the_workspace_bar(
    cx: &mut TestAppContext,
) {
    use crate::cloud_mark::{CloudBadge, indicator};
    use monocode_core::cloud_sessions::{CloudBackendKind, CloudState};
    let window = conversation(cx, PROJECT, true);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let draw = |visual: &mut gpui::VisualTestContext| {
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    draw(visual);
    // A local session keeps its checkout and has no cloud status.
    assert!(visual.debug_bounds("cloud-session-status").is_none());
    let badge = |kind, state| CloudBadge {
        kind,
        instance: "octo-abc".into(),
        indicator: indicator(Some(state)),
    };
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_badge(
                Some(badge(CloudBackendKind::Codespaces, CloudState::Stopped)),
                cx,
            );
            let shown = view.cloud_badge_for_test().unwrap();
            assert_eq!(shown.status(), "Codespace \u{b7} Stopped");
            assert_eq!(shown.tooltip(None), "Codespace \u{b7} octo-abc");
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-session-status").is_some());
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_badge(
                Some(badge(
                    CloudBackendKind::DockerSandbox,
                    CloudState::Available,
                )),
                cx,
            );
            assert_eq!(
                view.cloud_badge_for_test().unwrap().status(),
                "Docker Sandbox \u{b7} Available"
            );
            view.set_cloud_badge(None, cx);
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-session-status").is_none());
}

/// Saves a project cloud config (codespace part only) for the conversation's project.
fn save_codespace_config(view: &Conversation, base_ref: &str, machine: Option<&str>) {
    use monocode_core::cloud_sessions::project_cloud::{
        self, CodespaceProjectConfig, ProjectCloud,
    };
    project_cloud::save(
        &locked(&view.store).unwrap(),
        &view.summary.cwd,
        &ProjectCloud {
            codespace: CodespaceProjectConfig {
                base_ref: base_ref.into(),
                machine: machine.map(str::to_owned),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
}

#[gpui::test]
fn the_composer_starts_and_resets_from_the_saved_project_machine_and_branch(
    cx: &mut TestAppContext,
) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            save_codespace_config(view, "release", Some("standardLinux32gb"));
            view.set_machine_loader(Arc::new(|_, _| {
                Ok(vec![
                    machine("basicLinux32gb", "Basic"),
                    machine("standardLinux32gb", "Standard"),
                ])
            }));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.machine.as_deref(), Some("standardLinux32gb"));
            assert_eq!(draft.branch.read(cx).content.as_ref(), "release");
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            // Offered by the repository: it stays and the row shows it.
            assert_eq!(
                view.cloud_draft.as_ref().unwrap().machine.as_deref(),
                Some("standardLinux32gb")
            );
            assert_eq!(view.cloud_field_value(CloudField::Machine), "Standard");
            view.cloud_menu_choice("cloud-set:machine:basicLinux32gb", cx);
            set_text(view, |d| d.branch.clone(), "other", cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.machine.as_deref(), Some("standardLinux32gb"));
            assert_eq!(draft.branch.read(cx).content.as_ref(), "release");
        })
        .unwrap();
}

#[gpui::test]
fn an_empty_branch_field_starts_on_the_project_base_branch(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_codespace_config(view, "release", None);
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            set_text(view, |d| d.branch.clone(), "", cx);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(requests.lock().unwrap()[0].branch, "release");
}

#[gpui::test]
fn a_project_machine_the_repository_does_not_allow_falls_back_to_the_smallest_with_a_note(
    cx: &mut TestAppContext,
) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_codespace_config(view, "", Some("gone32gb"));
            view.set_machine_loader(Arc::new(|_, _| {
                // The devcontainer asks for 4 cores: no 2-core machine, and not the saved one.
                Ok(vec![
                    sized("premiumLinux", 8, 32),
                    sized("standardLinux32gb", 4, 16),
                ])
            }));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let draft = view.cloud_draft.as_ref().unwrap();
            // The saved machine is only a preference: the smallest allowed one is shown and
            // checked, with a note, and nothing counts as changed for this session.
            assert_eq!(draft.machine, None);
            let note = draft.machine_note.clone().unwrap();
            assert!(note.contains("gone32gb"), "{note}");
            assert!(note.contains("4 cores, 16 GB RAM"), "{note}");
            assert_eq!(
                view.cloud_field_value(CloudField::Machine),
                "4 cores, 16 GB RAM"
            );
            assert!(!draft.row_changed(CloudRow::Machine));
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    // No override: the start applies the same rule for the branch it creates on.
    assert_eq!(requests.lock().unwrap()[0].machine, None);
}

#[gpui::test]
fn a_failed_listing_still_offers_the_project_machine_the_row_shows(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_codespace_config(view, "", Some("premiumLinux"));
            view.set_machine_loader(Arc::new(|_, _| Err("gh is not installed".into())));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.cloud_field_value(CloudField::Machine), "premiumLinux");
            let (_, items) = view.cloud_field_menu(CloudField::Machine);
            assert_eq!(items.len(), 1);
            assert!(matches!(
                &items[0],
                MenuItem::Action(a)
                    if a.id.as_ref() == "cloud-set:machine:premiumLinux" && a.checked
            ));
            // Choosing it keeps the project machine: no override is sent, the start fills it
            // in from the project's settings.
            assert!(view.cloud_menu_choice("cloud-set:machine:premiumLinux", cx));
            assert!(
                !view
                    .cloud_draft
                    .as_ref()
                    .unwrap()
                    .row_changed(CloudRow::Machine)
            );
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(requests.lock().unwrap()[0].machine, None);
}

// ------------------------------------------------ uniform rows, defaults and session changes

fn docker_account(
    id: &str,
    docker_id: &str,
) -> monocode_core::docker_accounts::DockerAccountSummary {
    monocode_core::docker_accounts::DockerAccountSummary {
        id: id.into(),
        docker_id: docker_id.into(),
        status: monocode_core::docker_accounts::DockerAccountStatus::Valid,
        checked_at: None,
        expires_at: None,
        created_at: 1,
    }
}

fn labels(target: CloudTarget) -> Vec<&'static str> {
    CloudRow::of(target).iter().map(|row| row.label()).collect()
}

#[test]
fn both_backends_show_the_same_rows_in_the_same_order() {
    assert_eq!(
        labels(CloudTarget::Codespace),
        [
            "Machine",
            "Stop when idle",
            "Base branch",
            "Branch",
            "Claude account",
            "Claude setup"
        ]
    );
    assert_eq!(
        labels(CloudTarget::DockerSandbox),
        [
            "Size",
            "Runtime limit",
            "Base branch",
            "Branch",
            "Docker account",
            "Claude account",
            "Claude setup"
        ]
    );
    // Place, lifetime, branches, accounts, setup: the same slots, only the first two and the
    // Docker account differ by backend.
    let shared = |target| -> Vec<&'static str> {
        labels(target)
            .into_iter()
            .filter(|l| {
                ![
                    "Machine",
                    "Size",
                    "Stop when idle",
                    "Runtime limit",
                    "Docker account",
                ]
                .contains(l)
            })
            .collect()
    };
    assert_eq!(
        shared(CloudTarget::Codespace),
        shared(CloudTarget::DockerSandbox)
    );
}

#[gpui::test]
fn the_panel_renders_every_row_of_the_backend_without_a_status_column(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let draw = |visual: &mut gpui::VisualTestContext| {
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    draw(visual);
    for target in [CloudTarget::DockerSandbox, CloudTarget::Codespace] {
        window
            .update(visual, |view, _, cx| {
                view.clear_cloud_target(cx);
                view.cloud_menu_choice(
                    match target {
                        CloudTarget::Codespace => "workspace:cloud-codespace",
                        CloudTarget::DockerSandbox => "workspace:cloud-docker",
                    },
                    cx,
                );
            })
            .unwrap();
        draw(visual);
        for row in CloudRow::of(target) {
            assert!(visual.debug_bounds(row.id()).is_some(), "{:?}", row);
            // No status column: a row has no mark, whatever it holds.
            assert!(
                visual
                    .debug_bounds(Box::leak(format!("{}-mark", row.id()).into_boxed_str()))
                    .is_none(),
                "{row:?}"
            );
        }
        assert!(visual.debug_bounds("cloud-target-pill").is_none());
    }
}

fn changed_rows(view: &Conversation) -> Vec<CloudRow> {
    let draft = view.cloud_draft.as_ref().unwrap();
    CloudRow::of(draft.target)
        .iter()
        .copied()
        .filter(|row| draft.row_changed(*row))
        .collect()
}

#[gpui::test]
fn a_sandbox_row_is_marked_when_it_differs_and_clears_when_set_back(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            view.set_docker_accounts_loader(Arc::new(|_| {
                Ok(vec![docker_account("acct-1", "octo")])
            }));
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert!(changed_rows(view).is_empty());
            assert_eq!(changed_rows(view).len(), 0);
            // The project default is marked in the menu.
            let (_, sizes) = view.cloud_field_menu(CloudField::Size);
            let marked: Vec<_> = sizes
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Action(a) if a.description.is_some() => Some(a.label.to_string()),
                    _ => None,
                })
                .collect();
            assert_eq!(marked, ["2 CPUs, 4 GB"]);
            view.cloud_menu_choice("cloud-set:size:4:8g", cx);
            view.cloud_menu_choice("cloud-set:runtime:1h", cx);
            view.cloud_menu_choice("cloud-set:claude:personal", cx);
            view.cloud_menu_choice("cloud-set:docker:acct-1", cx);
            assert_eq!(
                changed_rows(view),
                [
                    CloudRow::Size,
                    CloudRow::Runtime,
                    CloudRow::DockerAccount,
                    CloudRow::Claude
                ]
            );
            assert_eq!(changed_rows(view).len(), 4);
            // Choosing the default again clears each mark.
            view.cloud_menu_choice("cloud-set:size:2:4g", cx);
            view.cloud_menu_choice("cloud-set:runtime:2h", cx);
            view.cloud_menu_choice("cloud-set:claude:", cx);
            view.cloud_menu_choice("cloud-set:docker:", cx);
            assert!(changed_rows(view).is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn a_codespace_row_is_marked_when_it_differs_and_clears_when_set_back(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            view.set_machine_loader(Arc::new(|_, _| {
                Ok(vec![
                    machine("basicLinux32gb", "Basic"),
                    machine("standardLinux32gb", "Standard"),
                ])
            }));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(changed_rows(view).is_empty());
            view.cloud_menu_choice("cloud-set:machine:standardLinux32gb", cx);
            view.cloud_menu_choice("cloud-set:idle:120", cx);
            view.cloud_menu_choice("cloud-set:claude:personal", cx);
            set_text(view, |d| d.base.clone(), "dev", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                changed_rows(view),
                [
                    CloudRow::Machine,
                    CloudRow::Idle,
                    CloudRow::Base,
                    CloudRow::Claude
                ]
            );
            view.cloud_menu_choice("cloud-set:machine:basicLinux32gb", cx);
            view.cloud_menu_choice("cloud-set:idle:30", cx);
            view.cloud_menu_choice("cloud-set:claude:", cx);
            set_text(view, |d| d.base.clone(), "main", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert!(changed_rows(view).is_empty()))
        .unwrap();
}

#[gpui::test]
fn an_untouched_panel_sends_no_overrides(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_codespace_config(view, "release", Some("standardLinux32gb"));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    let request = &sent[0];
    assert_eq!(request.branch, "release");
    assert_eq!(request.machine, None);
    assert_eq!(request.idle_timeout_minutes, None);
    assert_eq!(request.work_branch, None);
    assert_eq!(
        (request.account.as_deref(), request.account_pinned),
        (None, false)
    );
}

#[gpui::test]
fn a_codespace_request_carries_each_override(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_machine_loader(Arc::new(|_, _| {
                Ok(vec![
                    machine("basicLinux32gb", "Basic"),
                    machine("standardLinux32gb", "Standard"),
                ])
            }));
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.cloud_menu_choice("cloud-set:machine:standardLinux32gb", cx);
            view.cloud_menu_choice("cloud-set:idle:240", cx);
            view.cloud_menu_choice("cloud-set:claude:personal", cx);
            set_text(view, |d| d.base.clone(), "develop", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            // The working branch followed the base until another name is typed.
            assert_eq!(
                view.cloud_draft
                    .as_ref()
                    .unwrap()
                    .branch
                    .read(cx)
                    .content
                    .as_ref(),
                "develop"
            );
            set_text(view, |d| d.branch.clone(), "monoptah/cloud-setup", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| view.create_cloud(cx))
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    let request = &sent[0];
    assert_eq!(request.branch, "develop");
    assert_eq!(request.machine.as_deref(), Some("standardLinux32gb"));
    assert_eq!(request.idle_timeout_minutes, Some(240));
    assert_eq!(request.work_branch.as_deref(), Some("monoptah/cloud-setup"));
    // A changed Claude account is this session's own.
    assert_eq!(request.account.as_deref(), Some("personal"));
    assert!(request.account_pinned);
}

#[gpui::test]
fn a_codespace_choosing_the_secret_again_is_a_pinned_no_account_only_when_the_project_has_one(
    cx: &mut TestAppContext,
) {
    use monocode_core::cloud_sessions::project_cloud::{
        self, CodespaceProjectConfig, ProjectCloud,
    };
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            project_cloud::save(
                &locked(&view.store).unwrap(),
                &view.summary.cwd,
                &ProjectCloud {
                    codespace: CodespaceProjectConfig {
                        claude_account: Some("work".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            // The project's account is the default; switching to the secret is a change.
            assert!(changed_rows(view).is_empty());
            view.cloud_menu_choice("cloud-set:claude:", cx);
            assert_eq!(changed_rows(view), [CloudRow::Claude]);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent[0].account, None);
    assert!(
        sent[0].account_pinned,
        "an explicit 'no account' is pinned too"
    );
}

#[gpui::test]
fn the_working_branch_follows_the_base_and_stops_when_typed(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            set_text(view, |d| d.base.clone(), "a", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let branch = |view: &Conversation, cx: &gpui::App| {
        view.cloud_draft
            .as_ref()
            .unwrap()
            .branch
            .read(cx)
            .content
            .to_string()
    };
    window
        .update(cx, |view, _, cx| {
            assert_eq!(branch(view, cx), "a");
            set_text(view, |d| d.branch.clone(), "mine", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            set_text(view, |d| d.base.clone(), "b", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| assert_eq!(branch(view, cx), "mine"))
        .unwrap();
}

#[gpui::test]
fn an_invalid_working_branch_is_refused_inline_before_any_request(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            for bad in ["--x", "a b", "a..b", "x;y"] {
                set_text(view, |d| d.branch.clone(), bad, cx);
                view.create_cloud(cx);
                let draft = view.cloud_draft.as_ref().unwrap();
                assert!(!draft.creating, "{bad}");
                assert!(
                    draft
                        .error
                        .as_deref()
                        .is_some_and(|e| e.contains("invalid")),
                    "{bad}: {:?}",
                    draft.error
                );
            }
        })
        .unwrap();
    cx.run_until_parked();
    assert!(requests.lock().unwrap().is_empty());
}

#[gpui::test]
fn the_docker_account_row_lists_machine_sign_in_and_the_saved_accounts(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            view.set_docker_accounts_loader(Arc::new(|_| {
                Ok(vec![
                    docker_account("acct-1", "octo"),
                    docker_account("acct-2", "cat"),
                ])
            }));
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            // Still being read: the machine sign-in is the only choice.
            let (_, items) = view.cloud_field_menu(CloudField::DockerAccount);
            assert_eq!(items.len(), 1);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let (title, items) = view.cloud_field_menu(CloudField::DockerAccount);
            assert_eq!(title, "Docker account");
            let labels: Vec<_> = items
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Action(a) => Some(a.label.to_string()),
                    _ => None,
                })
                .collect();
            assert_eq!(labels, ["Machine sign-in (sbx login)", "octo", "cat"]);
            assert_eq!(
                view.cloud_field_value(CloudField::DockerAccount),
                "Machine sign-in (sbx login)"
            );
            view.cloud_menu_choice("cloud-set:docker:acct-2", cx);
            assert_eq!(view.cloud_field_value(CloudField::DockerAccount), "cat");
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    let config = sent[0].config.as_ref().unwrap();
    assert_eq!(config.docker_account.as_deref(), Some("acct-2"));
    // The Claude account was not touched.
    assert!(!sent[0].account_pinned);
}

#[gpui::test]
fn the_claude_row_names_the_secret_for_a_codespace_and_none_for_a_sandbox(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            assert_eq!(
                view.cloud_field_value(CloudField::Claude),
                "Codespaces secret (CLAUDE_CODE_OAUTH_TOKEN)"
            );
            view.clear_cloud_target(cx);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert_eq!(view.cloud_field_value(CloudField::Claude), "None");
        })
        .unwrap();
}

#[gpui::test]
fn the_workspace_menu_offers_existing_instance_and_cloud_settings(cx: &mut TestAppContext) {
    use monocode_core::cloud_sessions::project_cloud::DefaultBackend;
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let settings: Arc<Mutex<Vec<Option<DefaultBackend>>>> = Default::default();
    let existing: Arc<Mutex<Vec<(String, DefaultBackend)>>> = Default::default();
    let (a, b) = (settings.clone(), existing.clone());
    window
        .update(cx, |view, _, cx| {
            cx.subscribe(
                &cx.entity(),
                move |_, _, e: &OpenProjectCloudSettings, _| {
                    a.lock().unwrap().push(e.backend);
                },
            )
            .detach();
            cx.subscribe(&cx.entity(), move |_, _, e: &OpenExistingInstance, _| {
                b.lock().unwrap().push((e.cwd.clone(), e.backend));
            })
            .detach();
            view.set_offers_codespace(true, cx);
            let ids: Vec<_> = all_rows(view).into_iter().map(|row| row.0).collect();
            assert_eq!(
                ids,
                [
                    "workspace:cloud-codespace",
                    "workspace:cloud-docker",
                    "workspace:cloud-existing",
                    "workspace:cloud-settings"
                ]
            );
            // The settings item follows a separator.
            let items = view.cloud_menu_items();
            assert!(matches!(items[items.len() - 2], MenuItem::Separator));
            let rows = all_rows(view);
            assert_eq!(rows[0].3.as_deref(), Some("Billed to your GitHub account"));
            assert_eq!(rows[2].1, "Existing instance\u{2026}");
            assert_eq!(rows[3].1, "Cloud settings for this project\u{2026}");
            // Nothing chosen: the dialog opens on the project's default, existing on Codespace.
            assert!(view.cloud_menu_choice("workspace:cloud-settings", cx));
            assert!(view.cloud_menu_choice("workspace:cloud-existing", cx));
            // The backend that is chosen decides.
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert!(view.cloud_menu_choice("workspace:cloud-settings", cx));
            // No session of the project runs in a sandbox yet: nothing to pick, nothing emitted.
            let existing_item = |view: &Conversation| {
                view.cloud_menu_items().into_iter().find_map(|i| match i {
                    MenuItem::Action(a) if a.id.as_ref() == "workspace:cloud-existing" => {
                        Some((a.disabled, a.description.map(|d| d.to_string())))
                    }
                    _ => None,
                })
            };
            assert_eq!(
                existing_item(view),
                Some((true, Some("No sandbox yet".into())))
            );
            assert!(view.cloud_menu_choice("workspace:cloud-existing", cx));
            // A session bound to a sandbox makes it pickable, named on the item.
            add_cloud_session(view, CloudRef::sandbox("sbx-2"));
            view.forget_cloud_menu();
            assert_eq!(existing_item(view), Some((false, Some("sbx-2".into()))));
            assert!(view.cloud_menu_choice("workspace:cloud-existing", cx));
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            assert!(view.cloud_menu_choice("workspace:cloud-existing", cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *settings.lock().unwrap(),
        [None, Some(DefaultBackend::Sandbox)]
    );
    // Each request carries the project of the conversation.
    assert_eq!(
        *existing.lock().unwrap(),
        [
            (repo.clone(), DefaultBackend::Codespace),
            (repo.clone(), DefaultBackend::Sandbox),
            (repo, DefaultBackend::Codespace)
        ]
    );
}

/// Stores a (pending) cloud session of the conversation's project on `cloud`.
fn add_cloud_session(view: &Conversation, cloud: monocode_core::cloud_sessions::CloudRef) {
    use monocode_core::store::pending_cloud_project;
    locked(&view.store)
        .unwrap()
        .create_pending_cloud_session(
            "claude",
            pending_cloud_project(&view.summary.cwd, &cloud, "/workspaces/widgets"),
        )
        .unwrap();
}

#[gpui::test]
fn existing_instance_names_the_most_recent_codespace_of_the_project(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            assert_eq!(all_rows(view)[2].3, None);
            add_cloud_session(view, CloudRef::codespace("octo-abc"));
            // Read once: a new session shows when the shell says the project's sessions
            // changed, not on every opening of the menu.
            assert_eq!(all_rows(view)[2].3, None);
            assert!(view.cloud_menu_is_cached());
            view.forget_cloud_menu();
            assert!(!view.cloud_menu_is_cached());
            assert_eq!(all_rows(view)[2].3.as_deref(), Some("octo-abc"));
        })
        .unwrap();
}

#[gpui::test]
fn the_workspace_menu_reads_the_store_once_until_the_project_changes(cx: &mut TestAppContext) {
    use monocode_core::cloud_sessions::project_cloud::{self, DefaultBackend, ProjectCloud};
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            assert!(!view.cloud_menu_is_cached());
            assert_eq!(view.cloud_menu_items().len(), 7);
            assert!(view.cloud_menu_is_cached());
            // Saved behind the cache's back: the menu does not read the store again.
            project_cloud::save(
                &locked(&view.store).unwrap(),
                PROJECT,
                &ProjectCloud {
                    default_backend: Some(DefaultBackend::Sandbox),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                all_rows(view)[0].3.as_deref(),
                Some("Billed to your GitHub account")
            );
            assert_eq!(
                all_rows(view)[1].3.as_deref(),
                Some("Paid compute in your Docker account \u{b7} Set up first")
            );
            // The shell forgets it when the project's Cloud settings are saved.
            view.project_cloud_saved(cx);
            assert!(!view.cloud_menu_is_cached());
            // Only the billing note is shown.
            assert_eq!(
                all_rows(view)[0].3.as_deref(),
                Some("Billed to your GitHub account")
            );
        })
        .unwrap();
}

// ---------------------------------- project settings saved while the panel is open

fn sandbox_config(
    cpus: u32,
    memory: &str,
    ttl: &str,
    base: &str,
    claude: Option<&str>,
    docker: Option<&str>,
) -> DockerProjectConfig {
    DockerProjectConfig {
        repo_url: "https://github.com/octo/widgets.git".into(),
        cpus,
        memory: memory.into(),
        ttl: ttl.into(),
        base_ref: base.into(),
        claude_account: claude.map(str::to_owned),
        docker_account: docker.map(str::to_owned),
        ..Default::default()
    }
}

fn put_sandbox_config(view: &Conversation, config: &DockerProjectConfig) {
    docker_config::save(&locked(&view.store).unwrap(), &view.summary.cwd, config).unwrap();
}

/// Opens the Docker panel on a project saved with the "old" settings.
fn open_sandbox_panel(view: &mut Conversation, cx: &mut Context<Conversation>) {
    put_sandbox_config(
        view,
        &sandbox_config(2, "4g", "2h", "develop", Some("old"), Some("d-old")),
    );
    view.forget_project_cloud();
    view.cloud_menu_choice("workspace:cloud-docker", cx);
}

const NEW_SANDBOX: fn() -> DockerProjectConfig =
    || sandbox_config(4, "8g", "4h", "trunk", Some("new"), Some("d-new"));

#[gpui::test]
fn saving_the_project_settings_moves_the_rows_the_user_did_not_change(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            open_sandbox_panel(view, cx);
            // One row is changed for this session before the save.
            view.cloud_menu_choice("cloud-set:runtime:1h", cx);
            put_sandbox_config(view, &NEW_SANDBOX());
            // What the shell does when the project's Cloud settings are saved.
            view.project_cloud_saved(cx);
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!((draft.cpus, draft.memory.as_str()), (4, "8g"));
            assert_eq!(draft.account.as_deref(), Some("new"));
            assert_eq!(draft.docker_account.as_deref(), Some("d-new"));
            assert_eq!(draft.base.read(cx).content.as_ref(), "trunk");
            assert_eq!(draft.base_text, "trunk");
            // The override stays, the defaults are the new ones, and only it is an override.
            assert_eq!(draft.ttl, "1h");
            assert_eq!(draft.defaults.ttl, "4h");
            assert_eq!(changed_rows(view).len(), 1);
            assert!(draft.row_changed(CloudRow::Runtime));
            view.input
                .update(cx, |input, cx| input.set_text("go".into(), cx));
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let config = sent[0].config.as_ref().unwrap();
    assert_eq!((config.cpus, config.memory.as_str()), (4, "8g"));
    assert_eq!(config.ttl, "1h");
    assert_eq!(config.base_ref, "trunk");
    assert_eq!(config.claude_account.as_deref(), Some("new"));
    assert_eq!(config.docker_account.as_deref(), Some("d-new"));
    // The Claude account is the project's: not pinned to the sandbox.
    assert!(!sent[0].account_pinned);
}

#[gpui::test]
fn saving_the_project_settings_reads_the_docker_accounts_again(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            // The account is not in the list yet (it is added in the dialog that saves).
            view.set_docker_accounts_loader(Arc::new(|_| Ok(Vec::new())));
            put_sandbox_config(
                view,
                &sandbox_config(2, "4g", "2h", "develop", None, Some("acct-1")),
            );
            view.forget_project_cloud();
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            // A listed-but-missing account reads like the menu's entry, not as a raw id.
            assert_eq!(
                view.cloud_field_value(CloudField::DockerAccount),
                crate::cloud_presets::unavailable_account_label("acct-1")
            );
            view.set_docker_accounts_loader(Arc::new(|_| {
                Ok(vec![docker_account("acct-1", "octo")])
            }));
            view.project_cloud_saved(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.cloud_field_value(CloudField::DockerAccount), "octo");
        })
        .unwrap();
}

#[gpui::test]
fn a_claude_account_the_app_no_longer_has_reads_like_the_menu_entry(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            put_sandbox_config(
                view,
                &sandbox_config(2, "4g", "2h", "develop", Some("gone"), None),
            );
            view.forget_project_cloud();
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert_eq!(
                view.cloud_field_value(CloudField::Claude),
                crate::cloud_presets::unavailable_account_label("gone")
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_send_after_a_save_the_panel_never_heard_of_still_uses_the_new_settings(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            open_sandbox_panel(view, cx);
            view.cloud_menu_choice("cloud-set:docker:d-mine", cx);
            // Saved without the shell's notification (another window, say).
            put_sandbox_config(view, &NEW_SANDBOX());
            view.input
                .update(cx, |input, cx| input.set_text("go".into(), cx));
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    let config = sent[0].config.as_ref().unwrap();
    assert_eq!(
        (config.cpus, config.memory.as_str(), config.ttl.as_str()),
        (4, "8g", "4h")
    );
    assert_eq!(config.base_ref, "trunk");
    assert_eq!(config.claude_account.as_deref(), Some("new"));
    assert!(!sent[0].account_pinned);
    // The Docker account was changed for this session: it is the override, kept.
    assert_eq!(config.docker_account.as_deref(), Some("d-mine"));
}

#[gpui::test]
fn a_claude_account_changed_before_the_save_is_still_pinned_against_the_new_default(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx, PROJECT, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            open_sandbox_panel(view, cx);
            view.cloud_menu_choice("cloud-set:claude:mine", cx);
            put_sandbox_config(view, &NEW_SANDBOX());
            view.project_cloud_saved(cx);
            view.input
                .update(cx, |input, cx| input.set_text("go".into(), cx));
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(
        sent[0].config.as_ref().unwrap().claude_account.as_deref(),
        Some("mine")
    );
    assert!(sent[0].account_pinned);
    // A override equal to the NEW default is no override at all.
}

#[gpui::test]
fn choosing_the_new_default_after_a_save_clears_the_mark(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            open_sandbox_panel(view, cx);
            view.cloud_menu_choice("cloud-set:runtime:1h", cx);
            put_sandbox_config(view, &NEW_SANDBOX());
            view.project_cloud_saved(cx);
            assert_eq!(changed_rows(view).len(), 1);
            view.cloud_menu_choice("cloud-set:runtime:4h", cx);
            assert_eq!(changed_rows(view).len(), 0);
        })
        .unwrap();
}

#[gpui::test]
fn saving_the_codespace_settings_moves_the_untouched_rows_and_keeps_the_changed_ones(
    cx: &mut TestAppContext,
) {
    use monocode_core::cloud_sessions::project_cloud::{
        self, CodespaceProjectConfig, ProjectCloud,
    };
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    let save = |view: &Conversation, base: &str, machine: &str, idle: u32, claude: &str| {
        project_cloud::save(
            &locked(&view.store).unwrap(),
            &view.summary.cwd,
            &ProjectCloud {
                codespace: CodespaceProjectConfig {
                    base_ref: base.into(),
                    machine: Some(machine.into()),
                    idle_timeout_minutes: idle,
                    claude_account: Some(claude.into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    };
    window
        .update(cx, |view, _, cx| {
            save(view, "release", "basicLinux32gb", 30, "old");
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            // The idle stop is changed for this session; everything else follows the project.
            view.cloud_menu_choice("cloud-set:idle:240", cx);
            save(view, "trunk", "standardLinux32gb", 60, "new");
            view.project_cloud_saved(cx);
        })
        .unwrap();
    // The base field's change reaches the working branch field once the update is over.
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let draft = view.cloud_draft.as_ref().unwrap();
            assert_eq!(draft.machine.as_deref(), Some("standardLinux32gb"));
            assert_eq!(draft.account.as_deref(), Some("new"));
            assert_eq!(draft.base.read(cx).content.as_ref(), "trunk");
            // The working branch followed the base it was following.
            assert_eq!(draft.branch.read(cx).content.as_ref(), "trunk");
            assert_eq!(draft.idle, 240);
            assert_eq!(changed_rows(view).len(), 1);
            view.input
                .update(cx, |input, cx| input.set_text("go".into(), cx));
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].branch, "trunk");
    assert_eq!(sent[0].work_branch, None);
    // Not changed for this session: the project's machine and account apply.
    assert_eq!(sent[0].machine, None);
    assert_eq!(sent[0].account, None);
    assert!(!sent[0].account_pinned);
    assert_eq!(sent[0].idle_timeout_minutes, Some(240));
}

#[gpui::test]
fn the_working_branch_row_is_changed_when_it_names_another_branch(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            let changed = |view: &Conversation| {
                view.cloud_draft
                    .as_ref()
                    .unwrap()
                    .row_changed(CloudRow::Branch)
            };
            assert!(!changed(view));
            assert_eq!(changed_rows(view).len(), 0);
            set_text(view, |d| d.branch.clone(), "feature/x", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let changed = |view: &Conversation| {
                view.cloud_draft
                    .as_ref()
                    .unwrap()
                    .row_changed(CloudRow::Branch)
            };
            assert!(changed(view));
            assert_eq!(changed_rows(view).len(), 1);
            // Back to the base clears it; so does emptying the field (which means the base).
            set_text(view, |d| d.branch.clone(), "main", cx);
            set_text(view, |d| d.base.clone(), "main", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(
                !view
                    .cloud_draft
                    .as_ref()
                    .unwrap()
                    .row_changed(CloudRow::Branch)
            );
            set_text(view, |d| d.branch.clone(), "", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(
                !view
                    .cloud_draft
                    .as_ref()
                    .unwrap()
                    .row_changed(CloudRow::Branch)
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_docker_feature_branch_is_never_a_changed_row(cx: &mut TestAppContext) {
    let window = conversation(cx, PROJECT, false);
    window
        .update(cx, |view, _, cx| {
            open_sandbox_panel(view, cx);
            set_text(view, |d| d.branch.clone(), "feature/other", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(changed_rows(view).len(), 0);
        })
        .unwrap();
}

// ------------------------------------------------ a project that starts from a template

fn save_template_config(view: &Conversation, cpus: u32, memory: &str) {
    let config = DockerProjectConfig {
        repo_url: "https://github.com/octo/widgets.git".into(),
        environment: monocode_core::sandboxes::SandboxSource::Template {
            name: "rust-small".into(),
        },
        cpus,
        memory: memory.into(),
        ..Default::default()
    };
    docker_config::save(&locked(&view.store).unwrap(), &view.summary.cwd, &config).unwrap();
    view.forget_project_cloud();
}

#[gpui::test]
fn a_template_project_replaces_the_size_row_and_sends_the_template(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let draw = |visual: &mut gpui::VisualTestContext| {
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    window
        .update(visual, |view, _, cx| {
            save_template_config(view, 2, "4g");
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    draw(visual);
    // The row stays, but it only says where the size comes from.
    assert!(visual.debug_bounds("cloud-row-size").is_some());
    assert!(visual.debug_bounds("cloud-size-from-template").is_some());
    assert!(visual.debug_bounds("cloud-row-size-anchor").is_none());
    let row = visual.debug_bounds("cloud-row-size").unwrap();
    visual.simulate_click(row.center(), Default::default());
    draw(visual);
    window
        .update(visual, |view, _, _| {
            assert!(!view.menu_is(ComposerMenu::CloudField(CloudField::Size)));
        })
        .unwrap();
    // The other rows are untouched, and the template travels with the request.
    for id in ["cloud-row-runtime", "cloud-row-base", "cloud-row-branch"] {
        assert!(visual.debug_bounds(id).is_some(), "{id}");
    }
    window
        .update(visual, |view, _, cx| {
            set_text(view, |d| d.branch.clone(), "feature/x", cx);
            view.create_cloud(cx);
        })
        .unwrap();
    visual.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let config = sent[0].config.as_ref().unwrap();
    assert_eq!(config.environment.value(), Some("rust-small"));
    assert!(config.size_from_template() && !config.uses_devcontainer());
}

#[gpui::test]
fn the_size_row_of_a_template_project_is_never_a_change_for_this_session(cx: &mut TestAppContext) {
    // A saved size that is not a valid shape rides along with a template: no row, field or
    // stale draft value may turn it into an override the start then checks.
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let requests = watch(window, cx);
    window
        .update(cx, |view, _, cx| {
            save_template_config(view, 3, "5g");
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let draft = view.cloud_draft.as_mut().unwrap();
            assert!(draft.size_from_template());
            assert!(!draft.row_changed(CloudRow::Size));
            // Even a draft value that differs from the project's is not a changed row.
            draft.cpus = 8;
            draft.memory = "16g".into();
            assert!(!draft.row_changed(CloudRow::Size));
            assert!(
                CloudRow::of(draft.target)
                    .iter()
                    .all(|row| !draft.row_changed(*row))
            );
            // Nor does the devcontainer size warning apply to a size nobody uses.
            draft.devcontainer = monocode_core::cloud_sessions::project_cloud::DevcontainerInfo {
                host_cpus: Some(16),
                host_memory_mib: Some(64 * 1024),
                ..Default::default()
            };
            draft.cpus = 1;
            draft.memory = "2g".into();
            assert_eq!(draft.size_warning(), None);
            set_text(view, |d| d.branch.clone(), "feature/x", cx);
            view.create_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1, "the start was not refused");
    let config = sent[0].config.as_ref().unwrap();
    assert_eq!((config.cpus, config.memory.as_str()), (3, "5g"));
}

#[gpui::test]
fn the_devcontainer_size_warning_needs_a_built_devcontainer(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            save_template_config(view, 2, "4g");
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            let draft = view.cloud_draft.as_mut().unwrap();
            draft.devcontainer = monocode_core::cloud_sessions::project_cloud::DevcontainerInfo {
                host_cpus: Some(8),
                host_memory_mib: Some(16 * 1024),
                ..Default::default()
            };
            draft.cpus = 2;
            draft.memory = "4g".into();
            let mut config = draft.config.clone().unwrap();
            config.environment = monocode_core::sandboxes::SandboxSource::Image {
                reference: "rust:slim".into(),
            };
            // An image without the dev container: nothing is built, nothing to warn about.
            config.use_devcontainer = None;
            draft.config = Some(config.clone());
            assert_eq!(draft.size_warning(), None);
            // With the dev container built the size can be below what it asks for.
            config.use_devcontainer = Some(true);
            draft.config = Some(config);
            assert!(draft.size_warning().is_some());
        })
        .unwrap();
}

#[gpui::test]
fn a_default_project_keeps_the_size_menu(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            save_config(view);
            view.cloud_menu_choice("workspace:cloud-docker", cx);
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("cloud-row-size-anchor").is_some());
    assert!(visual.debug_bounds("cloud-size-from-template").is_none());
}
