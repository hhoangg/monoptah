use super::cloud_target::*;
use super::*;
use gpui::{TestAppContext, WindowHandle};
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
    assert!(visual.debug_bounds("cloud-target-panel").is_none());
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
    assert_eq!(size_label(2, "4g"), "2 vCPU / 4 GiB");
    assert_eq!(runtime_label("1h"), "1 hour then stop");
    assert_eq!(runtime_label("4h"), "4 hours then stop");
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
            let (_, compute) = view.cloud_field_menu(CloudField::Compute);
            let checked: Vec<_> = compute
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Action(a) if a.checked => Some(a.label.to_string()),
                    _ => None,
                })
                .collect();
            assert_eq!(checked, ["4 vCPU / 8 GiB"]);
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
            assert!(view.cloud_menu_choice("cloud-set:compute:4:8g", cx));
            assert!(view.cloud_menu_choice("cloud-set:runtime:1h", cx));
            assert!(view.cloud_menu_choice("cloud-set:account:personal", cx));
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
            view.set_machine_loader(Arc::new(move |project| {
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
            view.set_machine_loader(Arc::new(|_| Err("gh is not installed".into())));
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

#[gpui::test]
fn a_late_machine_listing_for_a_replaced_draft_is_ignored(cx: &mut TestAppContext) {
    let repo = temp_repo("ref: refs/heads/main\n");
    let window = conversation(cx, &repo, false);
    window
        .update(cx, |view, _, cx| {
            view.set_machine_loader(Arc::new(|_| Ok(vec![machine("a", "A")])));
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
        "cloud-row-compute",
        "cloud-row-runtime",
        "cloud-row-base",
        "cloud-row-branch",
        "cloud-target-create",
    ] {
        assert!(visual.debug_bounds(id).is_some(), "{id}");
    }
    // Collapsed: only the header stays.
    let header = visual.debug_bounds("cloud-target-header").unwrap();
    visual.simulate_click(header.center(), Default::default());
    draw(visual);
    assert!(visual.debug_bounds("cloud-target-header").is_some());
    assert!(visual.debug_bounds("cloud-row-compute").is_none());
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
    assert!(visual.debug_bounds("cloud-row-compute").is_none());
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
fn create_and_the_error_stay_visible_while_the_panel_is_collapsed(cx: &mut TestAppContext) {
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
        })
        .unwrap();
    draw(visual);
    let header = visual.debug_bounds("cloud-target-header").unwrap();
    visual.simulate_click(header.center(), Default::default());
    draw(visual);
    assert!(visual.debug_bounds("cloud-row-compute").is_none());
    let create = visual
        .debug_bounds("cloud-target-create")
        .expect("Create stays while collapsed");
    visual.simulate_click(create.center(), Default::default());
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
            view.set_machine_loader(Arc::new(move |_| {
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
