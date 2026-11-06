//! The Codespaces setup dialog opened from the project menu.
use super::*;
use crate::codespaces_dialog::{self, CodespacesDialog};

pub(super) struct OpenCodespaces {
    pub(super) view: Entity<CodespacesDialog>,
    _events: Subscription,
}

impl Shell {
    pub(super) fn open_codespaces_dialog(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_codespaces_dialog_with(path, codespaces_dialog::Env::system(), window, cx);
    }

    pub(super) fn open_codespaces_dialog_with(
        &mut self,
        path: String,
        env: codespaces_dialog::Env,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        self.close_preview(cx);
        if let Some(open) = self.codespaces.take() {
            open.view.update(cx, |view, cx| view.shutdown(cx));
        }
        let name = self.project_label(&path);
        let view = cx.new(|cx| CodespacesDialog::new(path, name, env, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            |this, _, _: &codespaces_dialog::Event, window, cx| {
                this.close_codespaces_dialog(window, cx)
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.codespaces = Some(OpenCodespaces {
            view,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_codespaces_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.codespaces.take() {
            open.view.update(cx, |view, cx| view.shutdown(cx));
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    pub(super) fn codespaces_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.codespaces.as_ref()?.view.clone();
        let viewport = window.viewport_size();
        let width = 460f32.min(viewport.width.as_f32() - 24.);
        let weak = cx.weak_entity();
        Some(
            div()
                .absolute()
                .size_full()
                .child(crate::modal::scrim("codespaces-scrim"))
                .child(
                    popover("codespaces-modal", Anchor::Center)
                        .modal_surface(self.workspace.boxy_layout())
                        .width(width)
                        .gap(0.)
                        .max_height(viewport.height.as_f32() - 32.)
                        .priority(60)
                        .on_dismiss(move |window, cx| {
                            let _ = weak
                                .update(cx, |this, cx| this.close_codespaces_dialog(window, cx));
                        })
                        .child(view),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tests::window;
    use gpui::{TestAppContext, WindowHandle};
    use monocode_core::codespaces::{GhOutput, GhRunner};
    use std::{path::Path, sync::Arc};

    struct Gh;
    impl GhRunner for Gh {
        fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
            let stdout = match args.first().map(String::as_str) {
                Some("auth") => "github.com\n  \u{2713} Logged in to github.com account octo (keyring)\n  - Token scopes: 'codespace', 'repo'\n".to_owned(),
                _ => r#"[{"secrets":[]}]"#.to_owned(),
            };
            Ok(GhOutput {
                success: true,
                stdout,
                stderr: String::new(),
            })
        }
    }

    fn env() -> codespaces_dialog::Env {
        codespaces_dialog::Env {
            runner: Arc::new(Gh),
            origin: |_: &Path| Some("git@github.com:acme/alpha.git".to_owned()),
            claude: || None,
        }
    }

    fn token(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> String {
        window
            .update(cx, |shell, _, cx| {
                shell
                    .codespaces
                    .as_ref()
                    .unwrap()
                    .view
                    .read(cx)
                    .first_field_text(cx)
            })
            .unwrap()
    }

    #[gpui::test]
    fn typing_and_pasting_reach_the_field_inside_the_shell_popover(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        cx.update(crate::input::bind);
        window
            .update(cx, |shell, w, cx| {
                shell.open_codespaces_dialog_with("/work/alpha".into(), env(), w, cx)
            })
            .unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let center = visual
            .debug_bounds("codespaces-field-CLAUDE_CODE_OAUTH_TOKEN")
            .expect("the field is drawn inside the shell")
            .center();
        visual.simulate_click(center, Default::default());
        visual.simulate_input("sk-ant");
        visual.write_to_clipboard(gpui::ClipboardItem::new_string("-pasted".into()));
        visual.simulate_keystrokes("cmd-v");
        visual.run_until_parked();
        assert_eq!(token(&window, cx), "sk-ant-pasted");
    }
}
