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
        crate::popover::bind(cx);
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
        "cloud-cancel",
        "cloud-save",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    // The footer holds only Cancel and Save: no summary line.
    assert!(visual.debug_bounds("cloud-summary").is_none());
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

fn children(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> (bool, bool) {
    window
        .read_with(cx, |shell, _| match &shell.cloud_settings {
            Some(open) => (true, open.fix.is_some()),
            None => (false, false),
        })
        .unwrap()
}

#[gpui::test]
fn the_fix_terminal_stacks_on_the_dialog_and_escape_closes_the_top_first(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert_eq!(children(&window, visual), (true, false));
    emit_on_dialog(
        &window,
        visual,
        cloud_settings::Event::Fix(FixSlot::new(FixCommand::run("Fix", "/bin/true", ["x"]))),
    );
    assert!(visual.debug_bounds("fix-terminal").is_some());
    assert_eq!(children(&window, visual), (true, true));
    escape(&window, visual);
    assert_eq!(children(&window, visual), (true, false));
    escape(&window, visual);
    assert_eq!(children(&window, visual), (false, false));
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
    assert_eq!(children(&window, cx), (true, true));
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
    assert_eq!(children(&window, cx), (false, false));
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
    assert_eq!(children(&window, visual), (false, false), "closed");
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
    assert_eq!(children(&window, visual), (true, false), "the dialog stays");
    escape(&window, visual);
    assert_eq!(children(&window, visual), (false, false));
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

fn bounds_of(visual: &mut VisualTestContext, selector: &'static str) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
}

fn open_sandbox_tab(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
    open_default(window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window((*window).into(), cx);
    click(visual, "cloud-seg-sandbox");
    visual.refresh().unwrap();
}

fn panel_of(
    window: &WindowHandle<Shell>,
    cx: &mut VisualTestContext,
) -> Option<gpui::Entity<crate::git_credential_panel::GitCredentialPanel>> {
    window
        .read_with(cx, |shell, cx| {
            shell
                .cloud_settings
                .as_ref()
                .unwrap()
                .view
                .read(cx)
                .credentials_panel()
        })
        .unwrap()
}

fn chosen_credential(window: &WindowHandle<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    window
        .read_with(cx, |shell, cx| {
            shell
                .cloud_settings
                .as_ref()
                .unwrap()
                .view
                .read(cx)
                .selected_credential()
        })
        .unwrap()
}

#[gpui::test]
fn the_segment_spans_the_dialog_and_the_default_row_sits_below_it(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let main = bounds_of(visual, "cloud-settings-main");
    let segments = bounds_of(visual, "cloud-segments");
    // The body pads 16px on each side; the segment fills what is left.
    assert!(
        (segments.size.width.as_f32() - (main.size.width.as_f32() - 32.)).abs() <= 1.,
        "segments {segments:?} main {main:?}"
    );
    let (left, right) = (
        bounds_of(visual, "cloud-seg-codespace"),
        bounds_of(visual, "cloud-seg-sandbox"),
    );
    assert!((left.size.width.as_f32() - right.size.width.as_f32()).abs() <= 1.);
    assert!(
        (left.size.width.as_f32() + right.size.width.as_f32() - segments.size.width.as_f32()).abs()
            <= 8.,
        "two halves fill the segment"
    );
    assert!(right.right().as_f32() <= segments.right().as_f32() + 1.);
    // The default select is on its own row below the segment, label and select on one line.
    let row = bounds_of(visual, "cloud-default-row");
    let select = bounds_of(visual, "cloud-select-default");
    assert!(row.top().as_f32() >= segments.bottom().as_f32());
    assert!(select.top().as_f32() >= row.top().as_f32());
    assert!(select.bottom().as_f32() <= row.bottom().as_f32() + 1.);
    assert!(select.size.width.as_f32() < segments.size.width.as_f32() / 2.);
}

#[gpui::test]
fn size_and_runtime_share_a_row_and_so_do_machine_and_idle(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Codespaces: Machine (once loaded) and Stop when idle.
    let machine = bounds_of(visual, "cloud-select-machine");
    let idle = bounds_of(visual, "cloud-select-idle");
    assert!((machine.top().as_f32() - idle.top().as_f32()).abs() <= 1.);
    assert!(machine.right() <= idle.left());
    click(visual, "cloud-seg-sandbox");
    visual.refresh().unwrap();
    let size = bounds_of(visual, "cloud-select-size");
    let runtime = bounds_of(visual, "cloud-select-runtime");
    assert!((size.top().as_f32() - runtime.top().as_f32()).abs() <= 1.);
    assert!(size.right() <= runtime.left());
    assert!(
        (size.size.width.as_f32() - runtime.size.width.as_f32()).abs() <= 1.,
        "two compact selects of one width"
    );
}

#[gpui::test]
fn manage_opens_a_side_panel_inside_the_dialog_and_close_restores_the_width(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let before = bounds_of(visual, "cloud-settings");
    assert!(panel_of(&window, visual).is_none());
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    assert!(panel_of(&window, visual).is_some());
    // No second modal: the panel is part of the dialog's own surface, and the shell holds
    // no credential dialog of its own.
    assert_eq!(children(&window, visual), (true, false));
    let after = bounds_of(visual, "cloud-settings");
    let panel = bounds_of(visual, "git-credential-panel");
    let side = bounds_of(visual, "cloud-credentials-side");
    // Structurally inside the one modal: the whole panel lies within the dialog surface,
    // which is the modal popover's only content and as wide as the popover was asked to be.
    assert!(
        panel.left() >= after.left()
            && panel.right() <= after.right() + gpui::px(1.)
            && panel.top() >= after.top()
            && panel.bottom() <= after.bottom() + gpui::px(1.),
        "panel {panel:?} dialog {after:?}"
    );
    let wanted = window
        .read_with(visual, |shell, cx| {
            shell
                .cloud_settings
                .as_ref()
                .unwrap()
                .view
                .read(cx)
                .dialog_width()
        })
        .unwrap();
    let viewport = visual.update(|window, _| window.viewport_size().width.as_f32());
    // The popover keeps a 1px margin on each side of its content.
    assert!(
        (after.size.width.as_f32() - wanted.min(viewport - 24.)).abs() <= 2.5,
        "dialog {after:?} popover width {wanted}"
    );
    assert!(
        after.size.width.as_f32() > before.size.width.as_f32(),
        "{before:?} {after:?}"
    );
    assert!(after.contains(&panel.center()));
    assert!(panel.left() >= side.left() && panel.right() <= side.right() + gpui::px(1.));
    assert!(
        side.left() >= bounds_of(visual, "cloud-settings-main").right() - gpui::px(1.),
        "the sidebar is on the right"
    );
    // Close returns to the normal width.
    click(visual, "git-cancel");
    visual.refresh().unwrap();
    assert!(panel_of(&window, visual).is_none());
    assert!(visual.debug_bounds("git-credential-panel").is_none());
    let restored = bounds_of(visual, "cloud-settings");
    assert!(
        (restored.size.width.as_f32() - before.size.width.as_f32()).abs() <= 1.,
        "{before:?} {restored:?}"
    );
    assert_eq!(children(&window, visual), (true, false), "dialog stays");
}

#[gpui::test]
fn typing_and_paste_reach_the_panel_and_escape_closes_the_panel_first(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    // The panel takes the keyboard when it opens: the first field is the profile name.
    visual.simulate_input("Work");
    click(visual, "git-field-token");
    visual.simulate_input("tok");
    visual.write_to_clipboard(gpui::ClipboardItem::new_string("_pasted".into()));
    visual.simulate_keystrokes("secondary-v");
    let panel = panel_of(&window, visual).unwrap();
    let (label, token) = panel.read_with(visual, |panel, cx| panel.typed(cx));
    assert_eq!(label, "Work");
    assert_eq!(token, "tok_pasted");
    // Escape closes the panel first and the typed token goes with it; the dialog stays.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(panel_of(&window, visual).is_none());
    assert_eq!(children(&window, visual), (true, false));
    assert_eq!(panel.read_with(visual, |p, cx| p.typed(cx).1), "");
    // The dialog has the keyboard back: a second Escape closes it.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(children(&window, visual), (false, false));
}

#[gpui::test]
fn a_saved_credential_becomes_the_selection_and_closes_the_panel(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert_eq!(chosen_credential(&window, visual), None);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    visual.simulate_input("Work");
    click(visual, "git-field-username");
    visual.simulate_input("me");
    click(visual, "git-field-token");
    visual.simulate_input("ghp_exampleTokenValue1234567890");
    click(visual, "git-save");
    visual.run_until_parked();
    visual.refresh().unwrap();
    let saved =
        monocode_core::git_credentials::GitCredentials::for_store(&fixture.store.lock().unwrap())
            .unwrap()
            .list()
            .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(
        chosen_credential(&window, visual).as_deref(),
        Some(saved[0].id.as_str())
    );
    assert!(panel_of(&window, visual).is_none(), "the choice is made");
    assert_eq!(children(&window, visual), (true, false));
}

#[gpui::test]
fn the_panel_belongs_to_the_docker_tab_and_the_dialog_close_ends_it(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    click(visual, "cloud-seg-codespace");
    visual.refresh().unwrap();
    assert!(panel_of(&window, visual).is_none());
    assert!(visual.debug_bounds("git-credential-panel").is_none());
    // Closing the whole dialog with the panel open also drops what was typed in it.
    click(visual, "cloud-seg-sandbox");
    visual.refresh().unwrap();
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    let panel = panel_of(&window, visual).unwrap();
    click(visual, "git-field-token");
    visual.simulate_input("secret-token");
    // The shell's Escape (the barrier path) closes the panel, then the dialog.
    escape(&window, visual);
    assert!(panel_of(&window, visual).is_none());
    assert_eq!(panel.read_with(visual, |p, cx| p.typed(cx).1), "");
    assert_eq!(
        children(&window, visual),
        (true, false),
        "panel then dialog"
    );
    escape(&window, visual);
    assert_eq!(children(&window, visual), (false, false));
}

fn set_requirements(window: &WindowHandle<Shell>, cx: &mut VisualTestContext) {
    // The devcontainer asks for far more than any listed machine or default size.
    window
        .update(cx, |shell, _, cx| {
            let view = shell.cloud_settings.as_ref().unwrap().view.clone();
            view.update(cx, |dialog, cx| {
                dialog.set_requirements(
                    monocode_core::cloud_sessions::project_cloud::DevcontainerInfo {
                        host_cpus: Some(64),
                        host_memory_mib: Some(512 * 1024),
                        ..Default::default()
                    },
                    cx,
                )
            });
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
}

#[gpui::test]
fn the_required_warnings_are_drawn_under_their_rows(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_requirements(&window, visual);
    // Codespaces: under the Machine and Stop when idle row.
    let (machine, idle, warning) = (
        bounds_of(visual, "cloud-select-machine"),
        bounds_of(visual, "cloud-select-idle"),
        bounds_of(visual, "cloud-machine-warning"),
    );
    assert!(warning.top() >= machine.bottom(), "{warning:?} {machine:?}");
    assert!(warning.top() >= idle.bottom(), "{warning:?} {idle:?}");
    assert!(visual.debug_bounds("cloud-size-warning").is_none());
    // Docker Sandbox: under the Size and Runtime limit row.
    click(visual, "cloud-seg-sandbox");
    visual.refresh().unwrap();
    let (size, runtime, warning) = (
        bounds_of(visual, "cloud-select-size"),
        bounds_of(visual, "cloud-select-runtime"),
        bounds_of(visual, "cloud-size-warning"),
    );
    assert!(warning.top() >= size.bottom(), "{warning:?} {size:?}");
    assert!(warning.top() >= runtime.bottom(), "{warning:?} {runtime:?}");
    assert!(visual.debug_bounds("cloud-machine-warning").is_none());
}

#[gpui::test]
fn the_account_change_note_is_drawn_once_under_the_credentials_on_each_tab(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // A debug selector names one element, and the note has no other selector: it is drawn
    // by the Credentials group alone, below its last row, on both tabs.
    let note = bounds_of(visual, "cloud-credentials-note");
    let git = bounds_of(visual, "cloud-git-codespace");
    assert!(note.top() >= git.bottom(), "{note:?} {git:?}");
    click(visual, "cloud-seg-sandbox");
    visual.refresh().unwrap();
    let note = bounds_of(visual, "cloud-credentials-note");
    let git = bounds_of(visual, "cloud-select-git");
    assert!(note.top() >= git.bottom(), "{note:?} {git:?}");
}

#[gpui::test]
fn no_caption_sits_between_a_field_and_the_next_label(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // The distance from one field's box to the next field's box is the next label plus the
    // column gaps; a caption under a field would add its height to one of them.
    let gap = |visual: &mut VisualTestContext, above: &'static str, below: &'static str| {
        bounds_of(visual, below).top().as_f32() - bounds_of(visual, above).bottom().as_f32()
    };
    let same = |gaps: &[f32]| {
        assert!(
            gaps.iter().all(|g| (g - gaps[0]).abs() <= 1.),
            "unequal gaps {gaps:?}"
        );
        // 12px label, 6px label gap, 12px column gap, with some slack for line heights.
        assert!(gaps[0] <= 44., "gap {gaps:?} holds more than a label");
    };
    same(&[
        gap(visual, "cloud-field-cs-base", "cloud-field-cs-devcontainer"),
        gap(
            visual,
            "cloud-field-cs-devcontainer",
            "cloud-select-machine",
        ),
    ]);
    click(visual, "cloud-seg-sandbox");
    visual.refresh().unwrap();
    same(&[
        gap(visual, "cloud-field-repo", "cloud-field-sb-base"),
        gap(visual, "cloud-field-sb-base", "cloud-field-sb-devcontainer"),
        gap(visual, "cloud-field-sb-devcontainer", "cloud-select-size"),
        gap(visual, "cloud-select-docker", "cloud-select-claude"),
        gap(visual, "cloud-select-claude", "cloud-select-git"),
    ]);
}

/// Saves a profile straight into the fixture's store.
fn save_profile(fixture: &Fixture) -> String {
    monocode_core::git_credentials::GitCredentials::for_store(&fixture.store.lock().unwrap())
        .unwrap()
        .save(
            "Work",
            monocode_core::git_credentials::GitPlatform::GitHub,
            "https://github.com",
            "me",
            "ghp_exampleTokenValue1234567890",
        )
        .unwrap()
}

fn profile_selector(id: &str) -> &'static str {
    Box::leak(format!("git-profile-{id}").into_boxed_str())
}

fn panel_has_focus(window: &WindowHandle<Shell>, cx: &mut VisualTestContext) -> bool {
    let panel = panel_of(window, cx).unwrap();
    window
        .update(cx, |_, w, cx| {
            use gpui::Focusable;
            panel.focus_handle(cx).is_focused(w)
        })
        .unwrap()
}

#[gpui::test]
fn a_panel_showing_a_saved_profile_takes_focus_itself_not_a_hidden_input(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    let id = save_profile(&fixture);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Pick the profile through the panel, then open the panel again on it.
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    click(visual, profile_selector(&id));
    click(visual, "git-save");
    visual.refresh().unwrap();
    assert_eq!(
        chosen_credential(&window, visual).as_deref(),
        Some(id.as_str())
    );
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    assert!(
        panel_has_focus(&window, visual),
        "the panel itself has the keyboard"
    );
    // The saved-profile form draws no input: typing goes nowhere, and Escape still reaches
    // the dialog through the panel.
    visual.simulate_input("zzz");
    let panel = panel_of(&window, visual).unwrap();
    assert_eq!(
        panel.read_with(visual, |p, cx| p.typed(cx)),
        (String::new(), String::new())
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(panel_of(&window, visual).is_none());
    assert_eq!(children(&window, visual), (true, false));
}

#[gpui::test]
fn closing_the_panel_leaves_the_keyboard_in_a_main_column_field(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    // The caret moves to a field of the dialog while the panel stays open.
    click(visual, "cloud-field-sb-base");
    visual.simulate_input("rel");
    // Escape closes the panel (a click on Close would move the focus itself).
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(panel_of(&window, visual).is_none());
    // The field keeps the keyboard: more typing lands there.
    visual.simulate_input("ease");
    let text = window
        .read_with(visual, |shell, cx| {
            shell
                .cloud_settings
                .as_ref()
                .unwrap()
                .view
                .read(cx)
                .text("sb_base", cx)
        })
        .unwrap();
    assert!(text.ends_with("release"), "{text}");
}

#[gpui::test]
fn closing_the_panel_reads_the_saved_list_again(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    let id = save_profile(&fixture);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    click(visual, profile_selector(&id));
    click(visual, "git-save");
    assert_eq!(
        chosen_credential(&window, visual).as_deref(),
        Some(id.as_str())
    );
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    // The profile goes away behind the open panel (a remove that ended too late to report
    // itself); closing the panel brings the dialog's list and selection up to date.
    monocode_core::git_credentials::GitCredentials::for_store(&fixture.store.lock().unwrap())
        .unwrap()
        .remove(&id)
        .unwrap();
    let ids = |window: &WindowHandle<Shell>, cx: &mut VisualTestContext| {
        window
            .read_with(cx, |shell, cx| {
                shell
                    .cloud_settings
                    .as_ref()
                    .unwrap()
                    .view
                    .read(cx)
                    .saved_credential_ids()
            })
            .unwrap()
    };
    assert_eq!(
        ids(&window, visual),
        std::slice::from_ref(&id),
        "stale until the panel closes"
    );
    click(visual, "git-cancel");
    visual.run_until_parked();
    assert!(ids(&window, visual).is_empty());
    assert_eq!(chosen_credential(&window, visual), None);
}

#[gpui::test]
fn escape_closes_the_confirm_remove_question_before_the_panel(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    let id = save_profile(&fixture);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    click(visual, profile_selector(&id));
    click(visual, "git-remove");
    let panel = panel_of(&window, visual).unwrap();
    assert!(panel.read_with(visual, |p, _| p.confirm_remove_asked()));
    escape(&window, visual);
    assert!(panel_of(&window, visual).is_some(), "the panel stays");
    assert!(!panel.read_with(visual, |p, _| p.confirm_remove_asked()));
    escape(&window, visual);
    assert!(panel_of(&window, visual).is_none());
    assert_eq!(children(&window, visual), (true, false), "the dialog stays");
    escape(&window, visual);
    assert_eq!(children(&window, visual), (false, false));
}

#[gpui::test]
fn removing_the_selected_profile_clears_the_dialogs_selection_and_list(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    let id = save_profile(&fixture);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    click(visual, profile_selector(&id));
    click(visual, "git-save");
    assert_eq!(
        chosen_credential(&window, visual).as_deref(),
        Some(id.as_str())
    );
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    click(visual, "git-remove");
    click(visual, "git-remove");
    visual.refresh().unwrap();
    assert_eq!(chosen_credential(&window, visual), None);
    assert!(panel_of(&window, visual).is_some(), "the panel stays open");
}

#[gpui::test]
fn the_panels_footer_sits_on_the_dialogs_footer_line(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    let (panel_button, dialog_button) = (
        bounds_of(visual, "git-save"),
        bounds_of(visual, "cloud-save"),
    );
    assert!(
        (panel_button.bottom().as_f32() - dialog_button.bottom().as_f32()).abs() <= 1.,
        "{panel_button:?} {dialog_button:?}"
    );
}

// ---------------------------------------------------------------- floating selects

fn dialog_state<R>(
    window: &WindowHandle<Shell>,
    cx: &mut VisualTestContext,
    read: impl FnOnce(&cloud_settings::CloudSettings, &gpui::App) -> R,
) -> R {
    window
        .read_with(cx, |shell, cx| {
            read(shell.cloud_settings.as_ref().unwrap().view.read(cx), cx)
        })
        .unwrap()
}

fn open_select_id(
    window: &WindowHandle<Shell>,
    cx: &mut VisualTestContext,
) -> Option<&'static str> {
    dialog_state(window, cx, |d, _| d.open_select_id())
}

fn select_label(window: &WindowHandle<Shell>, cx: &mut VisualTestContext, id: &str) -> String {
    dialog_state(window, cx, |d, cx| d.select_label(id, cx))
}

/// Where everything that must not move is, as far as it is drawn.
fn form_layout(
    visual: &mut VisualTestContext,
) -> Vec<(&'static str, Option<gpui::Bounds<gpui::Pixels>>)> {
    [
        "cloud-segments",
        "cloud-default-row",
        "cloud-select-default",
        "cloud-select-machine",
        "cloud-select-idle",
        "cloud-select-size",
        "cloud-select-runtime",
        "cloud-select-docker",
        "cloud-select-claude",
        "cloud-select-git",
        "cloud-git-manage",
        "cloud-credentials-note",
        "cloud-save",
        "cloud-settings-main",
        "cloud-settings",
    ]
    .into_iter()
    .map(|id| (id, visual.debug_bounds(id)))
    .collect::<Vec<_>>()
    .into_iter()
    .map({
        // Relative to the dialog, whose entrance still moves it a pixel or two.
        let origin = visual.debug_bounds("cloud-settings").map(|b| b.origin);
        move |(id, b)| {
            (
                id,
                b.zip(origin).map(|(mut b, origin)| {
                    b.origin = gpui::point(b.origin.x - origin.x, b.origin.y - origin.y);
                    b
                }),
            )
        }
    })
    .collect()
}

/// The same layout, give or take the half pixel the dialog's entrance and pixel snapping move
/// everything by. An inline list would push the rows below it down by its whole height.
fn assert_same_layout(
    now: &[(&'static str, Option<gpui::Bounds<gpui::Pixels>>)],
    before: &[(&'static str, Option<gpui::Bounds<gpui::Pixels>>)],
    what: &str,
) {
    assert_eq!(now.len(), before.len());
    for ((id, a), (_, b)) in now.iter().zip(before) {
        match (a, b) {
            (None, None) => {}
            (Some(a), Some(b)) => assert!(
                (a.origin.x - b.origin.x).abs() <= gpui::px(1.)
                    && (a.origin.y - b.origin.y).abs() <= gpui::px(1.)
                    && (a.size.width - b.size.width).abs() <= gpui::px(1.)
                    && (a.size.height - b.size.height).abs() <= gpui::px(1.),
                "{what}: {id} moved from {b:?} to {a:?}"
            ),
            _ => panic!("{what}: {id} appeared or went away"),
        }
    }
}

const FLOATING: &[&str] = &[
    "cloud-select-size",
    "cloud-select-runtime",
    "cloud-select-docker",
    "cloud-select-claude",
    "cloud-select-git",
];

#[gpui::test]
fn opening_a_select_floats_its_list_and_moves_nothing_in_the_form(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let before = form_layout(visual);
    assert!(FLOATING.iter().all(|id| visual.debug_bounds(id).is_some()));
    for id in FLOATING.iter().copied().chain(["cloud-select-default"]) {
        click(visual, id);
        visual.refresh().unwrap();
        assert_eq!(
            open_select_id(&window, visual),
            Some(id.trim_start_matches("cloud-select-")),
            "{id} opens"
        );
        assert_same_layout(&form_layout(visual), &before, id);
        // The list is a layer of its own: right under the trigger and as wide as it.
        let trigger = bounds_of(visual, id);
        let first = bounds_of(visual, "menu-action-0");
        assert!(
            first.top() >= trigger.bottom(),
            "{id}: {first:?} {trigger:?}"
        );
        assert!(
            first.top().as_f32() - trigger.bottom().as_f32() < 16.,
            "{id}: right under it"
        );
        assert!(
            (first.left().as_f32() - trigger.left().as_f32()).abs() <= 6.,
            "{id}: aligned to the trigger"
        );
        assert!(first.size.width.as_f32() <= trigger.size.width.as_f32() + 1.);
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert_eq!(open_select_id(&window, visual), None);
    }
    assert_same_layout(&form_layout(visual), &before, "after closing");
}

#[gpui::test]
fn the_list_of_a_select_overlaps_the_rows_below_and_leaves_them_in_place(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let docker = bounds_of(visual, "cloud-select-docker");
    let note = bounds_of(visual, "cloud-credentials-note");
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    // The Size list (five rows) is taller than the gap to the Docker account select.
    let last = bounds_of(visual, "menu-action-4");
    assert!(last.bottom() > docker.top(), "{last:?} {docker:?}");
    let (docker_now, note_now) = (
        bounds_of(visual, "cloud-select-docker"),
        bounds_of(visual, "cloud-credentials-note"),
    );
    assert!(
        (docker_now.top() - docker.top()).abs() <= gpui::px(1.),
        "{docker_now:?} {docker:?}"
    );
    assert!(
        (note_now.top() - note.top()).abs() <= gpui::px(1.),
        "{note_now:?} {note:?}"
    );
}

#[gpui::test]
fn the_codespaces_tab_floats_its_selects_too(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_default(&window, cx, "/work/monocode");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let before = form_layout(visual);
    for id in [
        "cloud-select-default",
        "cloud-select-machine",
        "cloud-select-idle",
        "cloud-select-claude",
    ] {
        click(visual, id);
        visual.refresh().unwrap();
        assert_eq!(
            open_select_id(&window, visual),
            Some(id.trim_start_matches("cloud-select-"))
        );
        assert_same_layout(&form_layout(visual), &before, id);
        assert!(bounds_of(visual, "menu-action-0").top() >= bounds_of(visual, id).bottom());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
    }
    // Stop when idle: pick 2 hours through the list.
    click(visual, "cloud-select-idle");
    visual.refresh().unwrap();
    let before_label = select_label(&window, visual, "idle");
    click(visual, "menu-action-1");
    assert_eq!(open_select_id(&window, visual), None);
    assert_ne!(select_label(&window, visual, "idle"), before_label);
    assert_same_layout(&form_layout(visual), &before, "after the pick");
    // Machine: pick another machine type through the list.
    click(visual, "cloud-select-machine");
    visual.refresh().unwrap();
    let before_label = select_label(&window, visual, "machine");
    click(visual, "menu-action-1");
    assert_eq!(open_select_id(&window, visual), None);
    assert_ne!(select_label(&window, visual, "machine"), before_label);
    assert_same_layout(&form_layout(visual), &before, "after the machine pick");
}

#[gpui::test]
fn picking_a_row_sets_the_value_and_closes_only_the_list(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert_eq!(select_label(&window, visual, "size"), "2 CPUs, 4 GB");
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    // The rows keep their labels, and the list sits above the dialog and takes the click.
    click(visual, "menu-action-3");
    assert_eq!(open_select_id(&window, visual), None, "the list closed");
    assert!(visual.debug_bounds("menu-action-0").is_none());
    assert_eq!(select_label(&window, visual, "size"), "8 CPUs, 16 GB");
    assert_eq!(children(&window, visual), (true, false), "the dialog stays");
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds("cloud-select-size").is_some(),
        "the trigger shows on"
    );
    // The dialog has the keyboard again: Escape closes it in one press.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(children(&window, visual), (false, false));
}

#[gpui::test]
fn the_keyboard_works_in_a_select_list_as_in_the_providers_menu(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    // The highlight starts on the current value (2 CPUs), so one Down then Enter is 4 CPUs.
    visual.simulate_keystrokes("down enter");
    visual.run_until_parked();
    assert_eq!(select_label(&window, visual, "size"), "4 CPUs, 8 GB");
    assert_eq!(open_select_id(&window, visual), None);
    assert_eq!(children(&window, visual), (true, false));
}

#[gpui::test]
fn escape_with_a_list_open_closes_only_the_list(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-select-runtime");
    visual.refresh().unwrap();
    let label = select_label(&window, visual, "runtime");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(open_select_id(&window, visual), None);
    assert!(visual.debug_bounds("menu-action-0").is_none());
    assert_eq!(
        select_label(&window, visual, "runtime"),
        label,
        "nothing was picked"
    );
    assert_eq!(children(&window, visual), (true, false), "the dialog stays");
    // The dialog has the keyboard: a second Escape closes it.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(children(&window, visual), (false, false));
}

#[gpui::test]
fn a_press_outside_the_list_closes_only_the_list(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Inside the dialog, on a control that would act on the press.
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    click(visual, "cloud-seg-codespace");
    assert_eq!(open_select_id(&window, visual), None);
    assert_eq!(
        tab_of(&window, visual),
        DefaultBackend::Sandbox,
        "the press reached nothing else"
    );
    assert_eq!(children(&window, visual), (true, false));
    // Outside the dialog, over the barrier: the list closes, the dialog stays.
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    visual.simulate_click(gpui::point(gpui::px(2.), gpui::px(2.)), Default::default());
    visual.run_until_parked();
    assert_eq!(open_select_id(&window, visual), None);
    assert_eq!(children(&window, visual), (true, false), "the dialog stays");
}

#[gpui::test]
fn a_press_on_a_trigger_closes_the_open_list_and_the_next_press_opens_that_triggers(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    click(visual, "cloud-select-size");
    assert_eq!(open_select_id(&window, visual), None, "the trigger toggles");
    // A press on another trigger is outside the open list: it closes it and nothing else.
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    click(visual, "cloud-select-runtime");
    assert_eq!(
        open_select_id(&window, visual),
        None,
        "the press only closes"
    );
    visual.refresh().unwrap();
    click(visual, "cloud-select-runtime");
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), Some("runtime"));
}

#[gpui::test]
fn the_lists_work_with_the_git_credential_panel_open(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    save_profile(&fixture);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-git-manage");
    visual.refresh().unwrap();
    assert!(panel_of(&window, visual).is_some());
    let before = form_layout(visual);
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), Some("size"));
    assert!(panel_of(&window, visual).is_some(), "the panel stays");
    assert_same_layout(&form_layout(visual), &before, "panel open");
    click(visual, "menu-action-0");
    assert_eq!(select_label(&window, visual, "size"), "1 CPU, 2 GB");
    assert!(
        panel_of(&window, visual).is_some(),
        "still open after a pick"
    );
    // Escape: the list first, then the panel, then the dialog.
    click(visual, "cloud-select-runtime");
    visual.refresh().unwrap();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(open_select_id(&window, visual), None);
    assert!(panel_of(&window, visual).is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(panel_of(&window, visual).is_none());
    assert_eq!(children(&window, visual), (true, false));
    // A press on Manage while a list is open only closes the list; it does not open the panel.
    click(visual, "cloud-select-git");
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), Some("git"));
    click(visual, "cloud-git-manage");
    assert_eq!(open_select_id(&window, visual), None);
    assert!(
        panel_of(&window, visual).is_none(),
        "the press reached nothing else"
    );
    assert_eq!(children(&window, visual), (true, false));
}

#[gpui::test]
fn the_git_list_reaches_past_the_dialog_card_and_still_takes_the_click(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    // Several saved profiles make the Git list tall (rows: the profiles, then "None").
    let credentials =
        monocode_core::git_credentials::GitCredentials::for_store(&fixture.store.lock().unwrap())
            .unwrap();
    let ids: Vec<String> = (0..7)
        .map(|n| {
            credentials
                .save(
                    &format!("Work {n}"),
                    monocode_core::git_credentials::GitPlatform::GitHub,
                    "https://github.com",
                    "me",
                    "ghp_exampleTokenValue1234567890",
                )
                .unwrap()
        })
        .collect();
    drop(credentials);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // The dialog card ends well above the bottom of the window, so a tall list reaches past it
    // (the window is tall enough to hold the dialog and the list).
    visual.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1300.)));
    visual.refresh().unwrap();
    assert_eq!(chosen_credential(&window, visual), None);
    click(visual, "cloud-select-git");
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), Some("git"));
    // The last saved profile is the row that lies furthest down (then comes "None").
    let row = bounds_of(visual, "menu-action-6");
    let card = bounds_of(visual, "cloud-settings");
    assert!(
        row.center().y > card.bottom(),
        "the premise: the row lies below the dialog card: {row:?} {card:?}"
    );
    let label_of_row = |visual: &mut VisualTestContext| {
        dialog_state(&window, visual, |d, cx| d.select_label("git", cx))
    };
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(open_select_id(&window, visual), None);
    let chosen = chosen_credential(&window, visual).expect("the click reached the row");
    assert!(ids.contains(&chosen));
    assert!(label_of_row(visual).starts_with("Work "));
    assert_eq!(
        children(&window, visual),
        (true, false),
        "a click on the list is not outside the dialog"
    );
}

#[gpui::test]
fn a_list_opened_before_the_docker_accounts_load_shows_them_once_they_do(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let on_dialog = |visual: &mut VisualTestContext,
                     f: fn(
        &mut cloud_settings::CloudSettings,
        &mut gpui::Context<cloud_settings::CloudSettings>,
    )| {
        window
            .update(visual, |shell, _, cx| {
                let view = shell.cloud_settings.as_ref().unwrap().view.clone();
                view.update(cx, f);
            })
            .unwrap();
    };
    // Back to before the accounts were read, then open the list.
    on_dialog(visual, |d, cx| d.forget_docker_accounts(cx));
    visual.refresh().unwrap();
    click(visual, "cloud-select-docker");
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), Some("docker"));
    assert!(visual.debug_bounds("menu-action-0").is_some());
    assert!(
        visual.debug_bounds("menu-action-1").is_none(),
        "only the machine sign-in before the accounts are read"
    );
    // The accounts arrive: the open list shows them and stays open.
    on_dialog(visual, |d, cx| d.reload_docker_accounts(cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), Some("docker"));
    assert!(visual.debug_bounds("menu-action-1").is_some());
    assert!(visual.debug_bounds("menu-action-2").is_some());
    // Picking a new row sets the account.
    click(visual, "menu-action-1");
    assert_eq!(open_select_id(&window, visual), None);
    assert_ne!(
        select_label(&window, visual, "docker"),
        crate::cloud_presets::MACHINE_SIGN_IN_LABEL
    );
}

#[gpui::test]
fn scrolling_the_dialog_body_closes_the_open_list(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-select-size");
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), Some("size"));
    // Over the body, away from the list.
    let at = bounds_of(visual, "cloud-segments").center();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-40.))),
        ..Default::default()
    });
    visual.run_until_parked();
    assert_eq!(open_select_id(&window, visual), None);
    assert_eq!(children(&window, visual), (true, false), "the dialog stays");
}

/// Opens `select` before its data is read, loads the data with `load`, presses Enter and returns
/// the (highlighted, checked) rows seen right after the load.
fn enter_after_a_late_load(
    window: &WindowHandle<Shell>,
    visual: &mut VisualTestContext,
    select: &'static str,
    forget: fn(
        &mut cloud_settings::CloudSettings,
        &mut gpui::Context<cloud_settings::CloudSettings>,
    ),
    load: fn(&mut cloud_settings::CloudSettings, &mut gpui::Context<cloud_settings::CloudSettings>),
) -> (Option<usize>, Option<usize>) {
    let on_dialog = |visual: &mut VisualTestContext,
                     f: fn(
        &mut cloud_settings::CloudSettings,
        &mut gpui::Context<cloud_settings::CloudSettings>,
    )| {
        window
            .update(visual, |shell, _, cx| {
                let view = shell.cloud_settings.as_ref().unwrap().view.clone();
                view.update(cx, f);
            })
            .unwrap();
    };
    on_dialog(visual, forget);
    visual.refresh().unwrap();
    click(visual, select);
    visual.refresh().unwrap();
    on_dialog(visual, load);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let rows = dialog_state(window, visual, |d, cx| d.open_select_rows(cx))
        .expect("the list stays open after the load");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(open_select_id(window, visual), None, "Enter picked a row");
    rows
}

#[gpui::test]
fn enter_in_a_docker_list_that_grew_keeps_the_checked_account(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, _, cx| {
            let view = shell.cloud_settings.as_ref().unwrap().view.clone();
            view.update(cx, |d, _| d.select_docker_account(Some("acct-2".into())));
        })
        .unwrap();
    let (highlighted, checked) = enter_after_a_late_load(
        &window,
        visual,
        "cloud-select-docker",
        |d, cx| d.forget_docker_accounts(cx),
        |d, cx| d.reload_docker_accounts(cx),
    );
    assert_eq!(checked, Some(2), "premise: the saved account is row 2");
    assert_eq!(highlighted, checked, "the checked row is highlighted");
    let account = dialog_state(&window, visual, |d, _| d.selected_docker_account());
    assert_eq!(account.as_deref(), Some("acct-2"), "Enter kept the account");
}

#[gpui::test]
fn enter_in_a_git_list_that_grew_keeps_the_checked_profile(cx: &mut TestAppContext) {
    let (window, fixture, _) = shell_with_fakes(cx);
    let credentials =
        monocode_core::git_credentials::GitCredentials::for_store(&fixture.store.lock().unwrap())
            .unwrap();
    for label in ["Work A", "Work B"] {
        credentials
            .save(
                label,
                monocode_core::git_credentials::GitPlatform::GitHub,
                "https://github.com",
                "me",
                "ghp_exampleTokenValue1234567890",
            )
            .unwrap();
    }
    drop(credentials);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let ids = dialog_state(&window, visual, |d, _| d.saved_credential_ids());
    assert_eq!(ids.len(), 2, "premise: both profiles are listed");
    let chosen = ids[1].clone();
    window
        .update(visual, |shell, _, cx| {
            let view = shell.cloud_settings.as_ref().unwrap().view.clone();
            view.update(cx, |d, _| d.select_credential(Some(chosen)));
        })
        .unwrap();
    let (highlighted, checked) = enter_after_a_late_load(
        &window,
        visual,
        "cloud-select-git",
        |d, cx| d.forget_credentials(cx),
        |d, cx| d.reload_credentials(None, cx),
    );
    assert!(checked.is_some(), "premise: a profile row is checked");
    assert_eq!(highlighted, checked, "the checked row is highlighted");
    assert_eq!(chosen_credential(&window, visual), Some(ids[1].clone()));
}

#[gpui::test]
fn scrolling_the_body_closes_the_list_and_the_field_keeps_the_keyboard(cx: &mut TestAppContext) {
    let (window, _, _) = shell_with_fakes(cx);
    open_sandbox_tab(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    click(visual, "cloud-field-repo");
    // Opened without a click: a click on the trigger would first move the keyboard to the
    // dialog, and the list would then give it back there.
    window
        .update(visual, |shell, window, cx| {
            let view = shell.cloud_settings.as_ref().unwrap().view.clone();
            view.update(cx, |d, cx| d.toggle_select_id("size", window, cx));
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), Some("size"));
    let at = bounds_of(visual, "cloud-segments").center();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-40.))),
        ..Default::default()
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(open_select_id(&window, visual), None);
    visual.simulate_input("zz");
    let text = dialog_state(&window, visual, |d, cx| d.text("repo", cx));
    assert!(
        text.contains("zz"),
        "typing reached the Repository field: {text}"
    );
}
