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
        self.close_menu(cx);
        self.close_preview(cx);
        if let Some(open) = self.codespaces.take() {
            open.view.update(cx, |view, cx| view.shutdown(cx));
        }
        let name = self.project_label(&path);
        let view =
            cx.new(|cx| CodespacesDialog::new(path, name, codespaces_dialog::Env::system(), cx));
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
