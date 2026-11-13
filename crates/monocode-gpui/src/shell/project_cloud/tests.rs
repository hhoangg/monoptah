use super::*;
use crate::cloud_settings::tests::{Fixture, JWT, https_origin};
use crate::cloud_settings::{self, EnvOverride, FixSlot};
use crate::conversation::cloud_target::{OpenDockerSetup, OpenProjectCloudSettings};
use crate::fix_terminal::FixCommand;
use crate::popover::MenuItem;
use crate::shell::tests::window;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use monocode_core::git_credentials::Secret;

fn project_items(
    shell: &mut Shell,
    path: &str,
    w: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<MenuItem> {
    let target = MenuTarget::Project {
        path: path.into(),
        rail: false,
    };
    shell.menu_items(&target, &mut Vec::new(), w, cx).1
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            MenuItem::Action(a) => Some(a.label.to_string()),
            _ => None,
        })
        .collect()
}

/// A shell whose Cloud settings dialogs reach only the fixture's fakes.
fn shell_with_fakes(cx: &mut TestAppContext) -> (WindowHandle<Shell>, Fixture, i64) {
    let (window, _, active) = window(cx);
    let fixture = Fixture::new();
    let env = fixture.env(https_origin);
    cx.update(|cx| {
        crate::input::bind(cx);
        cx.set_global(EnvOverride(env));
    });
    (window, fixture, active)
}

fn tab_of(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> DefaultBackend {
    window
        .read_with(cx, |shell, cx| {
            shell.cloud_settings.as_ref().unwrap().view.read(cx).tab()
        })
        .unwrap()
}

fn open_default(window: &WindowHandle<Shell>, cx: &mut TestAppContext, path: &str) {
    let path = path.to_owned();
    window
        .update(cx, |shell, w, cx| {
            shell.run_menu(
                MenuTarget::Project { path, rail: false },
                "cloud-settings",
                None,
                w,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn the_project_menu_has_one_cloud_settings_item_for_local_projects_only(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            let items = project_items(shell, "/work/monocode", w, cx);
            let names = labels(&items);
            assert!(
                !names.iter().any(|l| l.starts_with("Codespaces")
                    || l == "Cloud setup"
                    || l == "Docker Sandbox"),
                "{names:?}"
            );
            let at = names
                .iter()
                .position(|l| l == "Cloud settings…")
                .expect("Cloud settings");
            assert_eq!(names[at - 1], "Telegram…");
            let Some(MenuItem::Action(item)) = items
                .iter()
                .find(|i| matches!(i, MenuItem::Action(a) if a.id == "cloud-settings"))
            else {
                panic!("no cloud-settings item");
            };
            assert!(item.submenu.is_empty(), "it opens the dialog directly");
            let remote = project_items(shell, "remote://box/work/monocode", w, cx);
            assert!(
                !remote
                    .iter()
                    .any(|i| matches!(i, MenuItem::Action(a) if a.id == "cloud-settings"))
            );
        })
        .unwrap();
}

#[gpui::test]
fn the_menu_item_opens_the_dialog_and_draws_it(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    assert_eq!(tab_of(&window, cx), DefaultBackend::Codespace);
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for selector in [
        "cloud-settings",
        "cloud-seg-codespace",
        "cloud-seg-sandbox",
        "cloud-select-default",
        "cloud-recheck",
        "cloud-ready-gh",
        "cloud-field-cs-base",
        "cloud-select-claude",
        "cloud-git-codespace",
        "cloud-summary",
        "cloud-cancel",
        "cloud-save",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    // Cancel writes nothing and closes.
    let cancel = visual.debug_bounds("cloud-cancel").unwrap();
    visual.simulate_click(cancel.center(), Default::default());
    visual.run_until_parked();
    window
        .read_with(visual, |shell, _| assert!(shell.cloud_settings.is_none()))
        .unwrap();
    assert!(
        monocode_core::cloud_sessions::project_cloud::load(
            &fixture.store.lock().unwrap(),
            "/work/monocode"
        )
        .unwrap()
        .sandbox
        .is_none()
    );
}

#[gpui::test]
fn the_segment_switches_the_backend_and_draws_its_rows(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let sandbox = visual.debug_bounds("cloud-seg-sandbox").unwrap();
    visual.simulate_click(sandbox.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    for selector in [
        "cloud-field-repo",
        "cloud-select-size",
        "cloud-select-runtime",
        "cloud-select-docker",
        "cloud-git-manage",
        "cloud-ready-ssh",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(visual.debug_bounds("cloud-git-codespace").is_none());
    assert_eq!(tab_of(&window, visual), DefaultBackend::Sandbox);
}

#[gpui::test]
fn the_composer_docker_row_opens_cloud_settings_on_docker_sandbox(cx: &mut TestAppContext) {
    let (window, _, active) = shell_with_fakes(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&active].update(cx, |_, cx| {
                cx.emit(OpenDockerSetup("/work/monocode".into()))
            })
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(tab_of(&window, cx), DefaultBackend::Sandbox);
}

#[gpui::test]
fn a_conversation_can_ask_for_either_backend_or_the_default(cx: &mut TestAppContext) {
    let (window, _, active) = shell_with_fakes(cx);
    for (asked, expected) in [
        (Some(DefaultBackend::Sandbox), DefaultBackend::Sandbox),
        (Some(DefaultBackend::Codespace), DefaultBackend::Codespace),
        (None, DefaultBackend::Codespace),
    ] {
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&active].update(cx, |_, cx| {
                    cx.emit(OpenProjectCloudSettings {
                        cwd: "/work/monocode".into(),
                        backend: asked,
                    })
                })
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(tab_of(&window, cx), expected, "{asked:?}");
    }
}

fn emit_on_dialog(
    window: &WindowHandle<Shell>,
    cx: &mut TestAppContext,
    event: cloud_settings::Event,
) {
    window
        .update(cx, |shell, _, cx| {
            let view = shell.cloud_settings.as_ref().unwrap().view.clone();
            view.update(cx, |_, cx| cx.emit(event));
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
}

fn escape(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
    window
        .update(cx, |shell, w, cx| shell.cloud_settings_escape(w, cx))
        .unwrap();
    cx.run_until_parked();
}

fn children(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> (bool, bool, bool) {
    window
        .read_with(cx, |shell, _| match &shell.cloud_settings {
            Some(open) => (true, open.credential.is_some(), open.fix.is_some()),
            None => (false, false, false),
        })
        .unwrap()
}

#[gpui::test]
fn the_credential_dialog_and_the_fix_terminal_stack_and_escape_closes_the_top_first(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    emit_on_dialog(
        &window,
        cx,
        cloud_settings::Event::ManageCredentials {
            repo_url: "https://github.com/acme/alpha".into(),
            selected: None,
        },
    );
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("git-credential-dialog").is_some());
    assert_eq!(children(&window, visual), (true, true, false));
    escape(&window, visual);
    assert_eq!(children(&window, visual), (true, false, false));
    // The fix terminal is the topmost child.
    emit_on_dialog(
        &window,
        visual,
        cloud_settings::Event::Fix(FixSlot::new(FixCommand::run("Fix", "/bin/true", ["x"]))),
    );
    assert!(visual.debug_bounds("fix-terminal").is_some());
    assert_eq!(children(&window, visual), (true, false, true));
    escape(&window, visual);
    assert_eq!(children(&window, visual), (true, false, false));
    escape(&window, visual);
    assert_eq!(children(&window, visual), (false, false, false));
}

#[gpui::test]
fn the_fix_terminal_gets_the_token_only_in_its_environment(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    let command = FixCommand::run("Set up ssh", "/bin/true", ["--cloud", "setup", "ssh"])
        .with_env("DOCKER_ACCESS_TOKEN", Secret::new(JWT.into()));
    emit_on_dialog(
        &window,
        cx,
        cloud_settings::Event::Fix(FixSlot::new(command)),
    );
    window
        .update(cx, |shell, _, cx| {
            let fix = shell
                .cloud_settings
                .as_ref()
                .unwrap()
                .fix
                .as_ref()
                .expect("the fix terminal opened")
                .view
                .clone();
            fix.update(cx, |fix, cx| {
                assert_eq!(fix.command_line(), "true --cloud setup ssh");
                assert!(!fix.command_line().contains(JWT));
                fix.terminal().update(cx, |terminal, _| {
                    let spec = terminal.launched().unwrap();
                    // A wiped secret for the PTY, never a plain environment value.
                    assert!(spec.env_set.is_empty(), "{:?}", spec.env_set);
                    assert_eq!(
                        spec.secret_env,
                        [("DOCKER_ACCESS_TOKEN".to_owned(), JWT.to_owned())]
                    );
                    assert!(!spec.args.iter().any(|a| a.to_string_lossy().contains(JWT)));
                });
            });
        })
        .unwrap();
}

#[gpui::test]
fn the_fix_terminal_ending_checks_the_dialog_again(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    // The dialog asks for the fix itself so it remembers what to check again.
    window
        .update(cx, |shell, _, cx| {
            let view = shell.cloud_settings.as_ref().unwrap().view.clone();
            view.update(cx, |dialog, cx| {
                dialog.run_fix(
                    DefaultBackend::Codespace,
                    monocode_core::cloud_sessions::readiness::Fix::Command(
                        monocode_core::cloud_sessions::readiness::GH_LOGIN_COMMAND,
                    ),
                    cx,
                )
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(children(&window, cx), (true, false, true));
    let before = fixture.gh.calls.lock().unwrap().len();
    window
        .update(cx, |shell, _, cx| {
            let fix = shell.cloud_settings.as_ref().unwrap().fix.as_ref().unwrap();
            fix.view.update(cx, |fix, cx| {
                fix.terminal().update(cx, |t, cx| t.fake_exit(Some(0), cx))
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        fixture.gh.calls.lock().unwrap().len() > before,
        "the checklist was computed again"
    );
}

#[gpui::test]
fn saving_stores_the_settings_and_closes_every_layer(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    window
        .update(cx, |shell, _, cx| {
            let view = shell.cloud_settings.as_ref().unwrap().view.clone();
            view.update(cx, |dialog, cx| dialog.save(cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(children(&window, cx), (false, false, false));
    let saved = monocode_core::cloud_sessions::project_cloud::load(
        &fixture.store.lock().unwrap(),
        "/work/monocode",
    )
    .unwrap();
    assert!(
        saved.sandbox.is_none(),
        "an untouched sandbox stays unconfigured"
    );
}

fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    visual.simulate_click(bounds.center(), Default::default());
    visual.run_until_parked();
}

const TOKEN_FIELD: &str = "codespaces-field-CLAUDE_CODE_OAUTH_TOKEN";

fn token(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> String {
    window
        .read_with(cx, |shell, cx| {
            shell
                .cloud_settings
                .as_ref()
                .unwrap()
                .view
                .read(cx)
                .secret_text(cx)
        })
        .unwrap()
}

#[gpui::test]
fn typing_and_pasting_reach_the_secret_field_inside_the_shell_modal(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Tall enough for the whole dialog: the secret field is far down its body.
    visual.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(2400.)));
    visual.refresh().unwrap();
    click(visual, TOKEN_FIELD);
    visual.simulate_input("sk-ant");
    visual.write_to_clipboard(gpui::ClipboardItem::new_string("-pasted".into()));
    visual.simulate_keystrokes("secondary-v");
    visual.run_until_parked();
    assert_eq!(token(&window, visual), "sk-ant-pasted");
}

#[gpui::test]
fn the_footer_save_button_saves_the_typed_secret_over_stdin(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Tall enough for the whole dialog: the secret field is far down its body.
    visual.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(2400.)));
    visual.refresh().unwrap();
    click(visual, TOKEN_FIELD);
    visual.simulate_input("sk-ant-oat-footer");
    visual.run_until_parked();
    assert_eq!(fixture.gh.secret_calls(), 0, "typing alone saves nothing");
    click(visual, "cloud-save");
    visual.refresh().unwrap();
    assert_eq!(
        *fixture.gh.secret_sets.lock().unwrap(),
        [(
            "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
            "sk-ant-oat-footer".to_owned()
        )]
    );
    assert!(
        fixture
            .gh
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|argv| !argv.iter().any(|a| a.contains("sk-ant-oat-footer"))),
        "the token is never in argv"
    );
    assert_eq!(children(&window, visual), (false, false, false), "closed");
}

#[gpui::test]
fn escape_closes_an_open_select_before_the_dialog(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let open = |window: &WindowHandle<Shell>, visual: &mut VisualTestContext| {
        window
            .read_with(visual, |shell, cx| {
                shell
                    .cloud_settings
                    .as_ref()
                    .unwrap()
                    .view
                    .read(cx)
                    .select_open()
            })
            .unwrap()
    };
    click(visual, "cloud-select-default");
    visual.refresh().unwrap();
    assert!(open(&window, visual));
    assert!(visual.debug_bounds("cloud-select-default").is_some());
    escape(&window, visual);
    assert!(!open(&window, visual), "the select closed");
    assert_eq!(
        children(&window, visual),
        (true, false, false),
        "the dialog stays"
    );
    escape(&window, visual);
    assert_eq!(children(&window, visual), (false, false, false));
}

#[gpui::test]
fn saving_makes_the_projects_open_conversations_forget_their_cached_config(
    cx: &mut TestAppContext,
) {
    let (window, fixture, active) = shell_with_fakes(cx);
    // The active conversation is in /work/monocode; the other one is in another project.
    let other = window
        .update(cx, |shell, _, cx| {
            let other = *shell.sessions.keys().find(|id| **id != active).unwrap();
            for id in [active, other] {
                shell.sessions[&id].update(cx, |session, _| session.cache_docker_config());
                assert!(shell.sessions[&id].read(cx).docker_config_is_cached());
            }
            other
        })
        .unwrap();
    open_default(&window, cx, "/work/monocode");
    window
        .update(cx, |shell, _, cx| {
            let view = shell.cloud_settings.as_ref().unwrap().view.clone();
            view.update(cx, |dialog, cx| dialog.save(cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(fixture.gh.secret_calls(), 0);
    window
        .read_with(cx, |shell, cx| {
            assert!(
                !shell.sessions[&active].read(cx).docker_config_is_cached(),
                "the saved project's conversation forgot its copy"
            );
            assert!(
                shell.sessions[&other].read(cx).docker_config_is_cached(),
                "another project's conversation is left alone"
            );
        })
        .unwrap();
}
