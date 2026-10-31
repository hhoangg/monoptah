//! The board page: its toolbar, banners, menus and dialogs (`WorkflowBoard.tsx`, `NodePicker.tsx`).
//! The inspector that slides over the canvas is in `inspector.rs`.
use super::super::panels::OpenPicker;
use super::super::*;
use super::board::{BoardState, InspectorView};
use super::view::header_button;
use crate::popover::{Align, MenuAction, MenuItem};
use gpui::{Div, FontWeight, Stateful};
use monocode_core::mono::workflows::{
    board_edit::{REVIEW_FIRST, picker_groups, picker_unavailable, type_label},
    board_model::NodeType,
    model::NodeKind,
};

/// The icon of a node type in the picker.
pub(super) fn type_glyph(ty: NodeType) -> &'static str {
    match ty {
        NodeType::Schedule => "clock",
        NodeType::Manual => "play",
        NodeType::Session => "message-multiple",
        NodeType::Command => "terminal",
        NodeType::AskMono => "circle-help",
        NodeType::If | NodeType::Switch => "git-branch",
        NodeType::ForEach => "refresh-cw",
        NodeType::Report => "paper-plane",
    }
}

/// A small text button of a banner.
fn banner_button(id: &'static str, label: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Button)
        .aria_label(label)
        .flex_shrink_0()
        .rounded_sm()
        .px_1p5()
        .py_0p5()
        .text_color(content(0.7))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
        })
        .when(!enabled, |d| d.opacity(0.4))
        .child(label)
}

fn banner(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .w_full()
        .flex_shrink_0()
        .border_b_1()
        .border_color(content(0.1))
        .px_4()
        .py_1p5()
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(content(0.6))
}

impl Monos {
    /// The page of New workflow and Edit board.
    pub(in crate::monos) fn workflow_board_page(
        &mut self,
        mono: &Mono,
        route: &Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_workflows(cx);
        // The page is sliding away after Back: nothing to rebuild for it.
        if self.route.as_ref() != Some(route) && self.wf.board.is_none() {
            return div().size_full().into_any_element();
        }
        if !self.board_open(mono, route, cx) {
            let back = match route {
                Route::WorkflowBoard(id) => Some(Route::Workflow(id.clone())),
                _ => None,
            };
            return div()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .child(self.workflow_header("Edit board", back, cx))
                .into_any_element();
        }
        let Some(b) = self.wf.board.as_ref() else {
            return div().into_any_element();
        };
        let name = b.meta.name.clone();
        let can_save = b.can_save();
        let (add_anchor, more_anchor) = (b.add_anchor.clone(), b.more_anchor.clone());
        let name_field = self.board_field(
            super::board::BoardField::Name,
            "Workflow name",
            "Name",
            &name,
            false,
            Some(80),
            window,
            cx,
        );
        let header = div()
            .h(px(40.))
            .flex_shrink_0()
            .min_w_0()
            .border_b_1()
            .border_color(content(0.1))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .child(
                icon_button("mono-page-back", "Back", "chevron-left")
                    .debug_selector(|| "mono-page-back".into())
                    .on_click(cx.listener(|this, _, _, cx| this.board_request_back(cx))),
            )
            .child(
                div()
                    .id("wf-board-name")
                    .debug_selector(|| "wf-board-name".into())
                    .flex_1()
                    .min_w_0()
                    .child(name_field),
            )
            .child(
                div()
                    .relative()
                    .child(div().absolute().size_full().child(add_anchor.probe()))
                    .child(
                        icon_button("wf-board-add", "Add node", "plus")
                            .debug_selector(|| "wf-board-add".into())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.board_open_add_menu(window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .relative()
                    .child(div().absolute().size_full().child(more_anchor.probe()))
                    .child(
                        icon_button("wf-board-more", "More", "more-horizontal")
                            .debug_selector(|| "wf-board-more".into())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.board_open_more_menu(window, cx)
                            })),
                    ),
            )
            .child(
                header_button("wf-board-save", "Save", true, can_save).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if can_save {
                            this.board_save(false, false, false, cx);
                        }
                    },
                )),
            );
        let banners = self.board_banners(cx);
        let body = self.board_body(mono, window, cx);
        let leaving = self.board_leave_dialog(window, cx);
        let gate = self.run_gate_dialog(mono, window, cx);
        div()
            .id("wf-board")
            .debug_selector(|| "wf-board".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Cmd+S (Ctrl+S elsewhere) saves.
                if event.keystroke.modifiers.secondary()
                    && event.keystroke.key == "s"
                    && !window.has_pending_keystrokes()
                    && this.wf.board.as_ref().is_some_and(BoardState::can_save)
                {
                    this.board_save(false, false, false, cx);
                    cx.stop_propagation();
                }
            }))
            .child(header)
            .children(super::view::notice_line(
                "mono-workflow-notice",
                self.wf.page_notice.as_deref(),
            ))
            .children(banners)
            .child(body)
            .children(leaving)
            .children(gate)
            .into_any_element()
    }

    /// The notices under the header: changed elsewhere, review first, and the problems.
    fn board_banners(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(b) = self.wf.board.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let (saving, blocked) = (b.saving, b.blocked());
        if let Some((_, refused)) = &b.elsewhere {
            let refused = *refused;
            out.push(
                banner("wf-board-changed")
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_2()
                    .gap_y_1()
                    .role(Role::Status)
                    .child(div().flex_1().min_w_0().child(if refused {
                        "This workflow changed elsewhere, so your changes were not saved"
                    } else {
                        "This workflow changed elsewhere"
                    }))
                    .child(
                        banner_button("wf-board-reload", "Reload", true).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some((stored, _)) =
                                    this.wf.board.as_ref().and_then(|b| b.elsewhere.clone())
                                {
                                    this.board_load(stored, cx);
                                }
                            },
                        )),
                    )
                    .child(if refused {
                        banner_button("wf-board-overwrite", "Overwrite", !saving && !blocked)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !saving && !blocked {
                                    this.board_save(false, true, false, cx);
                                }
                            }))
                    } else {
                        banner_button("wf-board-keep", "Keep mine", true).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(b) = this.wf.board.as_mut() {
                                    b.elsewhere = None;
                                }
                                cx.notify();
                            },
                        ))
                    })
                    .into_any_element(),
            );
        }
        if b.unreviewed() {
            let none = b.issues.is_empty();
            out.push(
                banner("wf-board-review")
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(REVIEW_FIRST))
                    .child(
                        banner_button("wf-board-review-open", "Review", true).on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(id) = this.wf.board.as_ref().map(|b| b.base.id.clone())
                                {
                                    let back = this.route.clone();
                                    this.open_workflow_review(
                                        &id,
                                        super::ReviewIntent::Enable,
                                        back,
                                        cx,
                                    );
                                }
                            }),
                        ),
                    )
                    .when(none, |d| {
                        d.child(
                            banner_button("wf-board-save-paused", "Save paused", !saving).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.board_save(true, false, false, cx)
                                }),
                            ),
                        )
                    })
                    .into_any_element(),
            );
        }
        if !b.issues.is_empty() {
            let count = b.issues.len();
            let first = b.issues[0].message.clone();
            let open = b.show_issues;
            let enabled = b.meta.enabled;
            out.push(
                banner("wf-board-issues")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("wf-board-issues-toggle")
                                    .debug_selector(|| "wf-board-issues-toggle".into())
                                    .role(Role::Button)
                                    .aria_expanded(open)
                                    .cursor_pointer()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .size(px(6.))
                                            .flex_shrink_0()
                                            .rounded_full()
                                            .bg(gpui::Hsla::from(rgb(0xef4444)).opacity(0.7)),
                                    )
                                    .child(div().flex_shrink_0().text_color(content(0.8)).child(
                                        format!(
                                            "{count} {}",
                                            if count == 1 { "problem" } else { "problems" }
                                        ),
                                    ))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(content(0.45))
                                            .child(first),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(b) = this.wf.board.as_mut() {
                                            b.show_issues = !b.show_issues;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(enabled, |d| {
                                d.child(
                                    banner_button("wf-board-issues-paused", "Save paused", !saving)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.board_save(true, false, false, cx)
                                        })),
                                )
                            }),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .pl_3()
                                .pt_1p5()
                                .pb_0p5()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .children(b.issues.iter().map(|i| i.message.clone())),
                        )
                    })
                    .into_any_element(),
            );
        }
        out
    }

    /// The canvas, with the inspector over it and the loop being asked for on it.
    fn board_body(
        &mut self,
        mono: &Mono,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(b) = self.wf.board.as_ref() else {
            return div().into_any_element();
        };
        let (canvas, area, view, ask) = (
            b.canvas.clone(),
            b.area.clone(),
            b.view.clone(),
            b.loop_ask.clone(),
        );
        let inspector = view.map(|view| self.board_inspector(mono, view, window, cx));
        let loop_card = ask.map(|ask| self.board_loop_card(&ask, window, cx));
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(div().absolute().size_full().child(area.layout_probe()))
            .child(div().absolute().inset_0().child(canvas))
            .children(inspector)
            .children(loop_card.map(|card| {
                // A press outside the card closes it, as a popover does.
                div()
                    .absolute()
                    .inset_0()
                    .child(
                        div()
                            .id("wf-loop-scrim")
                            .debug_selector(|| "wf-loop-scrim".into())
                            .absolute()
                            .inset_0()
                            .occlude()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.board_cancel_loop(cx)),
                            ),
                    )
                    .child(card)
            }))
            .into_any_element()
    }

    /// "New loop": how many rounds and who decides, asked where the connection was let go.
    fn board_loop_card(
        &mut self,
        ask: &super::board::LoopAsk,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let origin = self
            .wf
            .board
            .as_ref()
            .and_then(|b| b.area.get())
            .map(|a| a.origin)
            .unwrap_or_default();
        let width = 296.;
        let area_width = self
            .wf
            .board
            .as_ref()
            .and_then(|b| b.area.get())
            .map_or(width + 16., |a| a.size.width.as_f32());
        let left = (ask.at.x - origin.x)
            .as_f32()
            .clamp(8., (area_width - width - 8.).max(8.));
        let top = (ask.at.y - origin.y).as_f32().max(8.);
        let target = self
            .wf
            .board
            .as_ref()
            .and_then(|b| b.node(&ask.to))
            .map(super::review::node_title)
            .unwrap_or_default();
        let form = self.board_loop_form(None, &ask.spec, window, cx);
        div()
            .id("wf-loop-ask")
            .debug_selector(|| "wf-loop-ask".into())
            .role(Role::Dialog)
            .aria_label("New loop")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(content(0.12))
            .bg(floating_fill())
            .occlude()
            .shadow(crate::popover::shadow())
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .text_color(content(0.7))
                    .child(format!("Go back to {target} until it is right.")),
            )
            .child(form)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .px_1()
                    .pt_2()
                    .child(
                        div()
                            .id("wf-loop-cancel")
                            .debug_selector(|| "wf-loop-cancel".into())
                            .role(Role::Button)
                            .aria_label("Cancel")
                            .cursor_pointer()
                            .rounded_md()
                            .px_3()
                            .py_1()
                            .text_size(px(12.))
                            .text_color(content(0.7))
                            .hover(|s| s.bg(content(0.08)))
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.board_cancel_loop(cx))),
                    )
                    .child(
                        div()
                            .id("wf-loop-add")
                            .debug_selector(|| "wf-loop-add".into())
                            .role(Role::Button)
                            .aria_label("Add loop")
                            .cursor_pointer()
                            .rounded_md()
                            .px_3()
                            .py_1()
                            .bg(content(0.1))
                            .text_size(px(12.))
                            .hover(|s| s.bg(content(0.14)))
                            .child("Add loop")
                            .on_click(cx.listener(|this, _, _, cx| this.board_confirm_loop(cx))),
                    ),
            )
            .into_any_element()
    }

    // ---- menus ------------------------------------------------------------------------------

    fn open_board_menu(
        &mut self,
        label: &'static str,
        items: Vec<MenuItem>,
        anchor: AnchorSlot,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = cx.new(|cx| Menu::new(label, items, window, cx).width(width));
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            this.picker = None;
            if let MenuEvent::Picked(id, _) = event {
                this.board_menu_picked(label, id.to_string(), cx);
            }
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(anchor),
            width,
            align: Align::End,
            _events: vec![picked],
        });
        cx.notify();
    }

    /// The menu behind "+ Node": every kind of node, grouped by its part in the workflow.
    pub(super) fn board_open_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let w = b.current();
        let has_trigger = w.nodes.iter().any(|n| n.is_trigger());
        let has_report = w
            .nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::Report { .. }));
        let anchor = b.add_anchor.clone();
        let mut items = Vec::new();
        for group in picker_groups() {
            items.push(MenuItem::Header(group.title.into()));
            for (ty, hint) in group.items {
                let why = picker_unavailable(*ty, has_trigger, has_report);
                items.push(
                    MenuAction::new(ty.name(), type_label(*ty))
                        .icon(type_glyph(*ty))
                        .description(why.unwrap_or(hint))
                        .disabled(why.is_some())
                        .into(),
                );
            }
        }
        self.open_board_menu("Add node", items, anchor, 248., window, cx);
    }

    pub(super) fn board_open_more_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let running = self.workflow_running(&b.base.id);
        let unsaved = b.is_new || b.changed();
        let anchor = b.more_anchor.clone();
        let enabled = b.meta.enabled;
        let run_hint = if unsaved {
            Some("Save first")
        } else if running {
            Some("Already running")
        } else {
            None
        };
        let mut run = MenuAction::new("run", "Run now")
            .icon("zap")
            .disabled(unsaved || running);
        if let Some(hint) = run_hint {
            run = run.description(hint);
        }
        let items = vec![
            MenuAction::new("settings", "Workflow settings")
                .icon("settings")
                .into(),
            MenuAction::new("export", "Export").icon("share").into(),
            run.into(),
            MenuAction::new("toggle", if enabled { "Pause" } else { "Resume" })
                .icon(if enabled { "queue-pause" } else { "queue-play" })
                .into(),
        ];
        self.open_board_menu("Workflow menu", items, anchor, 216., window, cx);
    }

    pub(super) fn board_menu_picked(&mut self, menu: &str, id: String, cx: &mut Context<Self>) {
        match (menu, id.as_str()) {
            ("Add node", name) => {
                if let Some(ty) = NodeType::ALL.into_iter().find(|t| t.name() == name) {
                    self.board_add(ty, cx);
                }
            }
            (_, "settings") => self.board_view(Some(InspectorView::Settings), cx),
            (_, "export") => {
                if let Some(w) = self.wf.board.as_ref().map(|b| b.current()) {
                    self.workflow_export(&w, cx);
                }
            }
            (_, "run") => self.board_run_now(cx),
            (_, "toggle") => self.board_toggle_enabled(cx),
            _ => {}
        }
    }

    // ---- leaving ----------------------------------------------------------------------------

    /// "Save your changes?"
    fn board_leave_dialog(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let b = self.wf.board.as_ref()?;
        if !b.leaving {
            return None;
        }
        let (blocked, saving) = (b.blocked(), b.saving);
        let text = if blocked {
            format!(
                "This workflow has changes you have not saved, and {}.",
                if b.issues.is_empty() {
                    "it needs a review before it can be enabled"
                } else {
                    "problems that keep it from being saved while enabled"
                }
            )
        } else {
            "This workflow has changes you have not saved.".to_string()
        };
        let size = window.viewport_size();
        let weak = cx.weak_entity();
        let width = 400f32.min(size.width.as_f32() - 24.);
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(crate::modal::scrim_color())
                                    .occlude(),
                            ),
                    )
                    .with_priority(79),
                )
                .child(
                    popover("wf-board-leave-modal", Anchor::Center)
                        .modal_surface(true)
                        .width(width)
                        .gap(0.)
                        .priority(80)
                        .on_dismiss(move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                if let Some(b) = this.wf.board.as_mut() {
                                    b.leaving = false;
                                }
                                cx.notify();
                            });
                        })
                        .child(
                            div()
                                .id("wf-board-leave")
                                .debug_selector(|| "wf-board-leave".into())
                                .role(Role::Dialog)
                                .aria_label("Save your changes?")
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(1.))
                                        .child("Save your changes?"),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .line_height(px(19.))
                                        .text_color(content(0.7))
                                        .child(text),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            crate::modal::ghost_button(
                                                "wf-board-keep-editing",
                                                "Keep editing",
                                            )
                                            .debug_selector(|| "wf-board-keep-editing".into())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    if let Some(b) = this.wf.board.as_mut() {
                                                        b.leaving = false;
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                        .child(
                                            crate::modal::ghost_button(
                                                "wf-board-discard",
                                                "Discard",
                                            )
                                            .debug_selector(|| "wf-board-discard".into())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.board_back(cx)),
                                            ),
                                        )
                                        .child(
                                            crate::modal::disabled(
                                                crate::modal::primary_button(
                                                    "wf-board-leave-save",
                                                    if blocked { "Save paused" } else { "Save" },
                                                )
                                                .debug_selector(|| "wf-board-leave-save".into()),
                                                saving,
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    if !saving {
                                                        this.board_save(blocked, false, true, cx);
                                                    }
                                                }),
                                            ),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
