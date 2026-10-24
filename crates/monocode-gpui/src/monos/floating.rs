use super::*;
use gpui::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, size};
struct Floating {
    owner: Entity<Monos>,
    selected: String,
    _observe: Subscription,
}
impl gpui::Render for Floating {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = self.owner.read(cx).owner.clone();
        // Defer until this root releases its render lease; the shell snapshots
        // all floating selections together with its own visible panes.
        let app: &mut App = cx;
        app.defer(move |cx| {
            let _ = shell.update(cx, |shell, cx| shell.keep_visible_idle_agents(cx));
        });
        #[cfg(any(not(target_os = "macos"), test))]
        let _ = &window;
        #[cfg(all(target_os = "macos", not(test)))]
        window.on_window_should_close(cx, |window, _| {
            crate::traffic_lights::hide(window);
            false
        });
        let owner = self.owner.read(cx);
        let selected = self.selected.clone();
        let chat = owner.chats.get(&selected).cloned();
        let roster = owner.roster.monos.clone();
        let mono = owner.roster.get(&selected).cloned();
        let parent = owner.parent_window;
        div()
            .size_full()
            .bg(rgb(CHAT))
            .text_color(content(1.))
            .font_family(crate::fonts::UI)
            .flex()
            .child(
                div()
                    .w(px(56.))
                    .h_full()
                    .flex_shrink_0()
                    .pt_7()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .border_r_1()
                    .border_color(content(0.1))
                    .children(roster.into_iter().map(|mono| {
                        let id = mono.id.clone();
                        div()
                            .id(SharedString::from(format!("floating-mono-{id}")))
                            .role(Role::Button)
                            .aria_label(mono.label())
                            .size(px(40.))
                            .rounded_lg()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(id == selected, |d| d.bg(content(0.1)))
                            .hover(|s| s.bg(content(0.08)))
                            .child(avatar::avatar(&mono.mascot, mono.color, "idle", 32., false))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.selected = id.clone();
                                this.owner.update(cx, |owner, cx| {
                                    owner.ensure_chat(&id, cx);
                                    if let Some(chat) = owner.chats.get(&id) {
                                        chat.update(cx, |v, _| v.mono_floating_window(window));
                                    }
                                });
                                cx.notify();
                            }))
                    }))
                    .child(
                        icon_button("floating-new-mono", "New mono", "plus").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.owner.update(cx, |owner, cx| {
                                    let projects = owner
                                        .current()
                                        .map(|m| m.projects.clone())
                                        .unwrap_or_default();
                                    owner.create(projects, window, cx);
                                    if let Some(id) = owner.active.clone() {
                                        this.selected = id.clone();
                                        if let Some(chat) = owner.chats.get(&id) {
                                            chat.update(cx, |v, _| v.mono_floating_window(window));
                                        }
                                    }
                                });
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .m_1p5()
                    .ml_0()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(pane_fill())
                    .when_some(mono, |d, mono| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_3()
                                .border_b_1()
                                .border_color(content(0.08))
                                .child(avatar::avatar(&mono.mascot, mono.color, "idle", 28., false))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(13.))
                                        .child(mono.label()),
                                )
                                .child(
                                    icon_button(
                                        "floating-reveal",
                                        "Open in MonoCode",
                                        "external-link",
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            let id = this.selected.clone();
                                            let owner = this.owner.clone();
                                            let app: &mut App = cx;
                                            app.defer(move |cx| {
                                                if let Some(parent) = parent {
                                                    let _ = parent.update(cx, |_, w, cx| {
                                                        owner
                                                            .update(cx, |m, cx| m.open(&id, w, cx));
                                                        w.activate_window();
                                                        cx.activate(true);
                                                    });
                                                }
                                            });
                                        },
                                    )),
                                )
                                .child(
                                    icon_button("floating-close", "Close floating chat", "close")
                                        .on_click(|_, window, _| {
                                            #[cfg(all(target_os = "macos", not(test)))]
                                            crate::traffic_lights::hide(window);
                                            #[cfg(any(not(target_os = "macos"), test))]
                                            window.remove_window();
                                        }),
                                ),
                        )
                    })
                    .when_some(chat, |d, chat| {
                        d.child(div().flex_1().min_h_0().child(chat))
                    }),
            )
    }
}
impl Monos {
    pub(crate) fn visible_idle_sessions(&self, cx: &App) -> Vec<i64> {
        let mut ids = self
            .chat()
            .into_iter()
            .map(|chat| chat.read(cx).summary.id)
            .collect::<Vec<_>>();
        for window in &self.floating {
            if let Some(window) = window.downcast::<Floating>()
                && let Ok(view) = window.read(cx)
                && let Some(chat) = self.chats.get(&view.selected)
            {
                ids.push(chat.read(cx).summary.id);
            }
        }
        ids
    }
    pub(super) fn open_floating(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        for existing in &self.floating {
            if let Some(existing) = existing.downcast::<Floating>()
                && existing
                    .update(cx, |view, window, cx| {
                        if view.selected != id {
                            return false;
                        }
                        window.activate_window();
                        cx.activate(true);
                        true
                    })
                    .unwrap_or(false)
            {
                return;
            }
        }
        let owner = cx.entity();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(440.), px(560.)),
                cx,
            ))),
            window_min_size: Some(size(px(360.), px(320.))),
            titlebar: Some(TitlebarOptions {
                title: Some(self.current().map(Mono::label).unwrap_or_default().into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        // Opening a window draws its root immediately. Release the manager's
        // mutable lease before Floating reads it during that first draw.
        let app: &mut App = cx;
        app.defer(move |cx| {
            let window_owner = owner.clone();
            let result = cx.open_window(options, |window, cx| {
                if let Some(chat) = window_owner.read(cx).chats.get(&id).cloned() {
                    chat.update(cx, |v, _| v.mono_floating_window(window));
                }
                cx.new(|cx| {
                    let observation = cx.observe(&window_owner, |_, _, cx| cx.notify());
                    Floating {
                        owner: window_owner,
                        selected: id,
                        _observe: observation,
                    }
                })
            });
            owner.update(cx, |owner, cx| {
                match result {
                    Ok(handle) => owner.floating.push(handle.into()),
                    Err(e) => owner.error = Some(e.to_string()),
                }
                cx.notify();
            });
        });
    }
    pub(super) fn open_workspace(&self, id: i64, window: &Window, cx: &mut Context<Self>) {
        let owner = self.owner.clone();
        let handle = window.window_handle();
        let app: &mut App = cx;
        app.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |owner, cx| owner.open(id, window, cx));
            });
        });
    }
    pub(crate) fn move_rail_drag(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some((_, start, over)) = &mut self.drag {
            if event.pressed_button != Some(MouseButton::Left) {
                self.finish_reorder(cx);
                return;
            }
            if (event.position - *start).magnitude() > 5. {
                self.skip_click = true;
                *over = self.rail_bounds.iter().find_map(|(id, s)| {
                    s.get()
                        .filter(|b| b.contains(&event.position))
                        .map(|_| id.clone())
                });
            }
        }
    }
}
