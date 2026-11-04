use super::*;
use crate::attachment_tray::{DropHooks, NO_FILE_DRAGGED, bind_drop, drop_overlay};

impl Conversation {
    /// Like the session drop region, accepts files over the transcript as well
    /// as the input. Both sources go through the same attachment validation.
    pub(super) fn attachment_drop_target(
        &mut self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        if !cx.has_active_drag() {
            self.file_drag = false;
        }
        let target = div()
            .id(id)
            .key_context("Session")
            .relative()
            .child(self.completion.read(cx).container_probe());
        bind_drop(
            target,
            DropHooks {
                over: |this: &Self| this.file_drag,
                set_over: Self::set_file_drag,
                drop: |this, paths, window, cx| {
                    if paths.is_empty() {
                        this.attachment_error = Some(NO_FILE_DRAGGED.into());
                        cx.notify();
                    } else {
                        this.add_attachments(paths, cx);
                    }
                    window.focus(&this.input.focus_handle(cx), cx);
                },
            },
            cx,
        )
    }

    fn set_file_drag(&mut self, over: bool, cx: &mut Context<Self>) {
        if self.file_drag != over {
            self.file_drag = over;
            cx.notify();
        }
    }

    pub(super) fn attachment_drop_overlay(&self, radius: f32) -> Option<AnyElement> {
        drop_overlay(self.file_drag, radius)
    }
}
