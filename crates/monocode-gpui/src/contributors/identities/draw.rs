//! Drawing the Identities section: the People column with its cards, and the
//! suggestions, unmapped and excluded cards beside it.
use super::{
    Suggestion,
    drag::DraggedIdentity,
    excluded_text,
    view::{Data, IdentitiesView, MenuKind},
};
use crate::{
    contributors::ui::{self, AvatarSize},
    popover::{Align, Anchor, Side, menu_popover},
    settings::widgets::secondary_button,
    theme::*,
};
use gpui::{
    AnyElement, Context, CursorStyle, Div, DragMoveEvent, ElementId, IntoElement, Render, Role,
    SharedString, Stateful, Window, canvas, div, prelude::*, px,
};
use monocode_core::contributors::model::{
    identity::MergeReason,
    palette::color_from_map,
    pipeline::ContributorsModel,
    unmapped::{ExcludedIdentity, PersonRow, UnmappedIdentity, UnmappedReason},
};

/// The two columns apply from this width (`@3xl/contrib`).
pub(in crate::contributors) const WIDE_AT: f32 = 768.;

pub(in crate::contributors) const MERGING_OFF: &str = "Merging is off: showing raw git identities. Turn on Merge identities in the header to apply this mapping.";
pub(in crate::contributors) const SAVED_TO: &str = "Saved to ";
pub(in crate::contributors) const SAVED_PATH: &str = ".monoptah/contributors.json";
pub(in crate::contributors) const SAVED_REST: &str = " in the repo (shared with the team).";
pub(in crate::contributors) const UNMAPPED_INTRO: &str = "Agents and machine accounts that no person lists yet. Add each to the person who owns it, or exclude it. Ordinary authors are not listed: each already counts as a person of their own, and can be excluded from the menu on their card.";
pub(in crate::contributors) const AGENT_NOTE: &str =
    "Agents are credited to the human (see How is the score calculated?)";

pub(in crate::contributors) fn reason_text(reason: MergeReason) -> &'static str {
    match reason {
        MergeReason::SameName => "Same name",
        MergeReason::SimilarEmail => "Similar email",
        MergeReason::GithubNoreply => "GitHub noreply",
    }
}

pub(in crate::contributors) fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// "3 identities · 5 commits".
pub(in crate::contributors) fn counts_text(row: &PersonRow) -> String {
    format!(
        "{} · {}",
        plural(row.identities.len() as u64, "identity", "identities"),
        plural(row.commits, "commit", "commits")
    )
}

/// "2 commits" with the machine-account note an unmapped identity can carry.
pub(in crate::contributors) fn unmapped_sub(item: &UnmappedIdentity) -> String {
    format!(
        "{}{}",
        plural(item.commits, "commit", "commits"),
        if item.reason == UnmappedReason::Machine {
            " · machine account"
        } else {
            ""
        }
    )
}

fn selector(id: String) -> impl Fn() -> String + 'static {
    move || id.clone()
}

fn element_id(text: &str) -> ElementId {
    SharedString::from(text.to_string()).into()
}

fn section() -> Div {
    div()
        .rounded_xl()
        .border_1()
        .border_color(content(0.1))
        .overflow_hidden()
}

fn head() -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(content(0.1))
        .px_4()
        .py(px(10.))
        .text_size(px(13.))
        .line_height(px(17.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(content(1.))
}

fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(content(0.45))
        .child(text.into())
}

fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(content(0.5))
        .child(text.into())
}

fn mono_key(text: String, alpha: f32) -> Div {
    div()
        .min_w_0()
        .truncate()
        .font_family(crate::fonts::mono())
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(content(alpha))
        .child(text)
}

/// SecondaryButton (the shared one) with the selector the tests find it by.
fn button(id: String, label: impl Into<SharedString>, disabled: bool) -> Stateful<Div> {
    secondary_button(element_id(&id), label, false, disabled).debug_selector(selector(id))
}

/// "AI agent" pill.
fn agent_mark(id: String) -> Div {
    div()
        .debug_selector(selector(id))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1()
        .rounded_full()
        .bg(content(0.08))
        .px(px(6.))
        .py(px(2.))
        .text_size(px(10.))
        .line_height(px(14.))
        .text_color(content(0.6))
        .child(icon("bot", 12.).text_color(content(0.6)))
        .child("AI agent")
}

/// A `...` button that opens a menu: the anchor probe sits inside it.
fn menu_trigger(
    id: String,
    label: String,
    side: f32,
    glyph: f32,
    slot: crate::popover::AnchorSlot,
    open: bool,
) -> Stateful<Div> {
    div()
        .id(element_id(&id))
        .debug_selector(selector(id))
        .role(Role::Button)
        .aria_label(label)
        .aria_expanded(open)
        .relative()
        .size(px(side))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(content(0.1)))
        .child(slot.probe())
        .child(icon("more-horizontal", glyph).text_color(content(0.5)))
}

/// The color of a person's avatar: the one the other tabs use.
pub(in crate::contributors) fn person_color(model: &ContributorsModel, id: &str) -> &'static str {
    color_from_map(&model.colors, id)
}

impl IdentitiesView {
    fn avatar_color(d: &Data, id: &str) -> &'static str {
        person_color(&d.derived.model, id)
    }

    fn people_section(&mut self, d: &Data, cx: &mut Context<Self>) -> AnyElement {
        let editable = d.editable();
        let cards: Vec<AnyElement> = d
            .derived
            .rows
            .iter()
            .map(|row| self.person_card(d, row, cx))
            .collect();
        section()
            .debug_selector(|| "contributors-id-people".into())
            .child(
                head()
                    .child("People")
                    .child(hint("rename a person, drag identities between people"))
                    .child(div().flex_1())
                    .child(
                        button("contributors-id-new-person".into(), "New person", !editable)
                            .child(icon("add-01", 12.).text_color(content(0.7)))
                            .child("New person")
                            .when(editable, |b| {
                                b.on_click(cx.listener(|this, _, _, cx| this.add_person(cx)))
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p_3()
                    .when(!d.merging(), |c| {
                        c.child(
                            div()
                                .id("contributors-id-merging-off")
                                .debug_selector(|| "contributors-id-merging-off".into())
                                .role(Role::Status)
                                .aria_label(MERGING_OFF)
                                .flex()
                                .items_start()
                                .gap_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(ui::amber(0.3))
                                .bg(ui::amber(0.05))
                                .px_3()
                                .py_2()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(content(0.7))
                                .child(
                                    icon("alert-circle", 14.)
                                        .mt(px(2.))
                                        .text_color(ui::amber(1.)),
                                )
                                .child(div().min_w_0().flex_1().child(MERGING_OFF)),
                        )
                    })
                    .when(d.derived.rows.is_empty(), |c| {
                        c.child(
                            muted("No people yet.")
                                .py_4()
                                .flex()
                                .justify_center()
                                .debug_selector(|| "contributors-id-no-people".into()),
                        )
                    })
                    .children(cards),
            )
            .child(
                div()
                    .debug_selector(|| "contributors-id-saved".into())
                    .border_t_1()
                    .border_color(content(0.1))
                    .px_4()
                    .py(px(10.))
                    .flex()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child(SAVED_TO)
                    .child(div().font_family(crate::fonts::mono()).child(SAVED_PATH))
                    .child(SAVED_REST),
            )
            .into_any_element()
    }

    fn person_card(&mut self, d: &Data, row: &PersonRow, cx: &mut Context<Self>) -> AnyElement {
        let editable = d.editable();
        let id = row.id.clone();
        let color = Self::avatar_color(d, &row.id);
        let highlighted = self.drag.highlights(row);
        let menu_open = matches!(
            &self.menu,
            Some(open) if open.kind == MenuKind::Person(row.id.clone())
        );
        let name = match self.names.get(&row.id) {
            Some(field) if editable => {
                let person = row.id.clone();
                div()
                    .debug_selector(selector(format!("contributors-id-name-{}", row.id)))
                    .min_w_0()
                    .flex_1()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .hover(|s| s.border_color(content(0.1)))
                    .px(px(6.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    // Escape leaves the field and puts the saved name back.
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                this.revert_name(&person, window, cx);
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .child(
                        div()
                            .debug_selector(selector(format!("contributors-id-field-{}", row.id)))
                            .w_full()
                            .child(field.input.clone()),
                    )
                    .into_any_element()
            }
            _ => div()
                .debug_selector(selector(format!("contributors-id-name-{}", row.id)))
                .min_w_0()
                .flex_1()
                .truncate()
                .px(px(6.))
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(content(1.))
                .child(row.name.clone())
                .into_any_element(),
        };
        let actions = editable.then(|| {
            let kind = MenuKind::Person(row.id.clone());
            let slot = self.slot(&super::view::anchor_id(&kind));
            menu_trigger(
                format!("contributors-id-actions-{}", row.id),
                format!("Actions for {}", row.name),
                24.,
                16.,
                slot,
                menu_open,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_menu(kind.clone(), window, cx);
            }))
        });
        let identities: Vec<AnyElement> = row
            .identities
            .iter()
            .map(|identity| self.identity_row(d, row, &identity.key, identity.agent, cx))
            .collect();
        let drop_id = id.clone();
        let move_id = id.clone();
        let leave_id = id.clone();
        div()
            .id(element_id(&format!("contributors-id-person-{id}")))
            .debug_selector(selector(format!("contributors-id-person-{id}")))
            .rounded_xl()
            .border_1()
            .px_3()
            .py(px(10.))
            .map(|d| {
                if highlighted {
                    d.border_color(accent_color().opacity(0.6))
                        .bg(accent_color().opacity(0.1))
                } else {
                    d.border_color(content(0.1)).bg(content(0.03))
                }
            })
            .when(editable, |card| {
                card.on_drag_move::<DraggedIdentity>(cx.listener(
                    move |this, event: &DragMoveEvent<DraggedIdentity>, _, cx| {
                        let inside = event.bounds.contains(&event.event.position);
                        if inside && this.drag.over() != Some(move_id.as_str()) {
                            this.drag.hover(Some(&move_id));
                            cx.notify();
                        } else if !inside && this.drag.over() == Some(leave_id.as_str()) {
                            this.drag.leave(&leave_id);
                            cx.notify();
                        }
                    },
                ))
                .on_drop(cx.listener(move |this, _: &DraggedIdentity, _, cx| {
                    this.dropped(&drop_id, cx);
                }))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(ui::avatar(&row.name, color, AvatarSize::Md))
                    .child(name)
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child(counts_text(row)),
                    )
                    .children(actions),
            )
            .child(div().mt_2().flex().flex_col().gap_1().children(identities))
            .when(editable, |card| {
                card.child(
                    div()
                        .debug_selector(selector(format!("contributors-id-drophint-{id}")))
                        .mt(px(6.))
                        .flex()
                        .justify_center()
                        .rounded_md()
                        .border_1()
                        .border_dashed()
                        .border_color(content(0.1))
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child("Drop an identity here"),
                )
            })
            .into_any_element()
    }

    fn identity_row(
        &mut self,
        d: &Data,
        person: &PersonRow,
        key: &str,
        agent: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editable = d.editable();
        let dims = self.drag.dims(key);
        let handle = self.row_focus.get(key).cloned();
        let grab = {
            let label = mono_key(key.to_string(), 0.8).flex_1();
            let body = div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .flex_1()
                .when(editable, |g| {
                    g.child(icon("grip-vertical", 14.).text_color(content(0.35)))
                })
                .child(label);
            if editable {
                let payload = DraggedIdentity {
                    key: key.to_string(),
                    from: person.id.clone(),
                };
                let weak = cx.weak_entity();
                let focus = self.focus.clone();
                body.id(element_id(&format!("contributors-id-grab-{key}")))
                    .debug_selector(selector(format!("contributors-id-grab-{key}")))
                    .cursor(CursorStyle::OpenHand)
                    .on_drag(payload, move |drag, _, window, cx| {
                        // The gesture is ours from here: Escape reaches this view.
                        let _ = weak.update(cx, |this, cx| {
                            this.drag.start(&drag.key, &drag.from);
                            cx.notify();
                        });
                        window.focus(&focus, cx);
                        cx.new(|_| drag.clone())
                    })
                    .into_any_element()
            } else {
                body.into_any_element()
            }
        };
        let identity_menu = editable.then(|| {
            let kind = MenuKind::Identity {
                person: person.id.clone(),
                key: key.to_string(),
            };
            let open = matches!(&self.menu, Some(m) if m.kind == kind);
            let slot = self.slot(&super::view::anchor_id(&kind));
            menu_trigger(
                format!("contributors-id-actions-identity-{key}"),
                format!("Actions for identity {key}"),
                20.,
                14.,
                slot,
                open,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_menu(kind.clone(), window, cx);
            }))
        });
        let move_menu = editable.then(|| {
            let kind = MenuKind::Move {
                key: key.to_string(),
            };
            let open = matches!(&self.menu, Some(m) if m.kind == kind);
            let slot = self.slot(&super::view::anchor_id(&kind));
            div()
                .id(element_id(&format!("contributors-id-move-{key}")))
                .debug_selector(selector(format!("contributors-id-move-{key}")))
                .role(Role::Button)
                .aria_label(format!("Move {key} to another person"))
                .aria_expanded(open)
                .relative()
                .flex_shrink_0()
                .rounded_md()
                .px(px(6.))
                .py(px(2.))
                .text_size(px(11.))
                .text_color(content(0.5))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
                .child(slot.probe())
                .child("Move to...")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_menu(kind.clone(), window, cx);
                }))
        });
        div()
            .id(element_id(&format!("contributors-id-row-{key}")))
            .debug_selector(selector(format!("contributors-id-row-{key}")))
            .when_some(handle, |row, handle| row.track_focus(&handle))
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .bg(content(0.05))
            .px_2()
            .py_1()
            .when(dims, |row| row.opacity(0.4))
            .child(grab)
            .when(agent, |row| {
                row.child(agent_mark(format!("contributors-id-agent-{key}")))
            })
            .children(identity_menu)
            .children(move_menu)
            .into_any_element()
    }

    fn suggestions_card(&mut self, d: &Data, cx: &mut Context<Self>) -> AnyElement {
        let locked = d.locked;
        let body: AnyElement = if !d.merging() {
            div()
                .id("contributors-id-suggestions-off")
                .debug_selector(|| "contributors-id-suggestions-off".into())
                .role(Role::Status)
                .aria_label(MERGING_OFF)
                .px_4()
                .py(px(20.))
                .text_size(px(12.))
                .text_color(content(0.6))
                .child(MERGING_OFF)
                .into_any_element()
        } else if d.derived.suggestions.is_empty() {
            muted("No suggestions")
                .debug_selector(|| "contributors-id-no-suggestions".into())
                .px_4()
                .py(px(20.))
                .flex()
                .justify_center()
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(
                    d.derived
                        .suggestions
                        .iter()
                        .enumerate()
                        .map(|(i, item)| self.suggestion(d, i, item, locked, cx)),
                )
                .into_any_element()
        };
        section()
            .debug_selector(|| "contributors-id-suggestions".into())
            .child(
                head()
                    .child("Suggested merges")
                    .child(hint("same name / similar email")),
            )
            .child(body)
            .into_any_element()
    }

    fn suggestion(
        &mut self,
        d: &Data,
        index: usize,
        item: &Suggestion,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sid = item.suggestion.id.clone();
        let keep = item
            .people
            .first()
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let people = item.people.iter().enumerate().map(|(i, p)| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .when(i > 0, |s| {
                    s.child(div().text_color(content(0.4)).child("↔"))
                })
                .child(ui::avatar(
                    &p.name,
                    Self::avatar_color(d, &p.id),
                    AvatarSize::Sm,
                ))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(p.name.clone()),
                )
        });
        let keys = item
            .people
            .iter()
            .flat_map(|p| p.identities.iter().map(|i| i.key.clone()))
            .collect::<Vec<_>>()
            .join(" · ");
        let (merge_id, dismiss_id) = (sid.clone(), sid.clone());
        div()
            .debug_selector(selector(format!("contributors-id-suggestion-{sid}")))
            .when(index > 0, |s| s.border_t_1().border_color(content(0.1)))
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_2()
                    .gap_y_1()
                    .text_size(px(13.))
                    .text_color(content(1.))
                    .children(people),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child(reason_text(item.suggestion.reason)),
            )
            .child(
                div()
                    .font_family(crate::fonts::mono())
                    .text_size(px(11.))
                    .text_color(content(0.65))
                    .child(keys),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .pt(px(2.))
                    .child(
                        button(
                            format!("contributors-id-merge-{sid}"),
                            format!("Merge into {keep}"),
                            locked,
                        )
                        .child(format!("Merge into {keep}"))
                        .when(!locked, |b| {
                            b.on_click(cx.listener(move |this, _, _, cx| {
                                this.merge_suggestion(&merge_id, cx)
                            }))
                        }),
                    )
                    .child(
                        button(format!("contributors-id-dismiss-{sid}"), "Dismiss", locked)
                            .child("Dismiss")
                            .when(!locked, |b| {
                                b.on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.dismiss(&dismiss_id, cx)
                                    }),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn unmapped_card(&mut self, d: &Data, cx: &mut Context<Self>) -> AnyElement {
        let unmapped = &d.derived.model.unmapped;
        let excluded = &d.derived.excluded;
        let body: AnyElement = if unmapped.is_empty() {
            muted("Nothing needs attention: no unowned agents or machine accounts.")
                .debug_selector(|| "contributors-id-nothing".into())
                .px_4()
                .py(px(20.))
                .flex()
                .justify_center()
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(
                    unmapped
                        .iter()
                        .enumerate()
                        .map(|(i, item)| self.unmapped_row(d, i, item, cx)),
                )
                .into_any_element()
        };
        let excluded_block = (!excluded.is_empty()).then(|| {
            let open = self.show_excluded;
            div()
                .border_t_1()
                .border_color(content(0.1))
                .child(
                    div()
                        .id("contributors-id-excluded-toggle")
                        .debug_selector(|| "contributors-id-excluded-toggle".into())
                        .role(Role::Button)
                        .aria_label(format!("Excluded ({})", excluded.len()))
                        .aria_expanded(open)
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px_4()
                        .py_2()
                        .text_size(px(12.))
                        .text_color(content(0.6))
                        .cursor_pointer()
                        .hover(|s| s.text_color(content(1.)))
                        .child(
                            icon(
                                if open {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                14.,
                            )
                            .text_color(content(0.6)),
                        )
                        .child(format!("Excluded ({})", excluded.len()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_excluded = !this.show_excluded;
                            cx.notify();
                        })),
                )
                .when(open, |block| {
                    block.child(
                        div()
                            .border_t_1()
                            .border_color(content(0.1))
                            .flex()
                            .flex_col()
                            .children(
                                excluded
                                    .iter()
                                    .enumerate()
                                    .map(|(i, item)| self.excluded_row(d, i, item, cx)),
                            ),
                    )
                })
        });
        section()
            .debug_selector(|| "contributors-id-unmapped-card".into())
            .child(
                head()
                    .items_baseline()
                    .child("Unmapped identities")
                    .child(hint(unmapped.len().to_string())),
            )
            .child(
                div()
                    .border_b_1()
                    .border_color(content(0.1))
                    .px_4()
                    .py_2()
                    .text_size(px(11.))
                    .line_height(px(18.))
                    .text_color(content(0.5))
                    .child(UNMAPPED_INTRO),
            )
            .child(body)
            .children(excluded_block)
            .into_any_element()
    }

    fn unmapped_row(
        &mut self,
        d: &Data,
        index: usize,
        item: &UnmappedIdentity,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locked = d.locked;
        let key = item.key.clone();
        let add = d.merging().then(|| {
            let kind = MenuKind::Add { key: key.clone() };
            let open = matches!(&self.menu, Some(m) if m.kind == kind);
            let slot = self.slot(&super::view::anchor_id(&kind));
            div()
                .id(element_id(&format!("contributors-id-add-{key}")))
                .debug_selector(selector(format!("contributors-id-add-{key}")))
                .role(Role::Button)
                .aria_label(format!("Add {key} to a person"))
                .aria_expanded(open)
                .relative()
                .flex_shrink_0()
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .px(px(10.))
                .py_1()
                .text_size(px(12.))
                .text_color(content(0.7))
                .map(|b| {
                    if locked {
                        b.opacity(0.4)
                    } else {
                        b.cursor_pointer()
                            .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
                    }
                })
                .child(slot.probe())
                .child("Add to person...")
                .when(!locked, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        this.open_menu(kind.clone(), window, cx);
                    }))
                })
        });
        let exclude_key = key.clone();
        div()
            .debug_selector(selector(format!("contributors-id-unmapped-{key}")))
            .when(index > 0, |r| r.border_t_1().border_color(content(0.1)))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .gap_y_1()
            .px_4()
            .py_2()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(mono_key(key.clone(), 0.8))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(unmapped_sub(item)),
                    ),
            )
            .map(|r| {
                if item.agent {
                    r.child(agent_mark(format!("contributors-id-unmapped-agent-{key}")))
                        .child(
                            div()
                                .w_full()
                                .debug_selector(selector(format!(
                                    "contributors-id-agent-note-{key}"
                                )))
                                .text_size(px(11.))
                                .text_color(content(0.5))
                                .child(AGENT_NOTE),
                        )
                } else {
                    r.child(
                        button(format!("contributors-id-exclude-{key}"), "Exclude", locked)
                            .child("Exclude")
                            .when(!locked, |b| {
                                b.on_click(cx.listener(move |this, _, _, cx| {
                                    this.exclude_key(&exclude_key, cx)
                                }))
                            }),
                    )
                }
            })
            .children(add)
            .into_any_element()
    }

    fn excluded_row(
        &mut self,
        d: &Data,
        index: usize,
        item: &ExcludedIdentity,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locked = d.locked;
        let key = item.key.clone();
        let include_key = key.clone();
        div()
            .debug_selector(selector(format!("contributors-id-excluded-{key}")))
            .when(index > 0, |r| r.border_t_1().border_color(content(0.1)))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .gap_y_1()
            .px_4()
            .py_2()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(mono_key(key.clone(), 0.65))
                    .children(excluded_text(item).map(|text| {
                        div()
                            .debug_selector(selector(format!("contributors-id-why-{key}")))
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(text)
                    })),
            )
            .when(item.removable, |r| {
                r.child(
                    button(
                        format!("contributors-id-include-{key}"),
                        "Include again",
                        locked,
                    )
                    .child("Include again")
                    .when(!locked, |b| {
                        b.on_click(
                            cx.listener(move |this, _, _, cx| this.include(&include_key, cx)),
                        )
                    }),
                )
            })
            .into_any_element()
    }
}

impl Render for IdentitiesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A release over nothing ends the gesture without a drop event.
        if self.drag.is_active() && !cx.has_active_drag() {
            self.drag.cancel();
        }
        let root = div()
            .id("contributors-identities")
            .debug_selector(|| "contributors-body-identities".into())
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.drag.is_active() {
                    this.cancel_drag(window, cx);
                    cx.stop_propagation();
                }
            }));
        let Some(d) = self.data(cx) else {
            return root;
        };
        self.sync_names(&d, window, cx);
        // One focus handle per identity row, so a moved row can take the focus.
        self.row_focus.retain(|key, _| {
            d.derived
                .rows
                .iter()
                .any(|row| row.identities.iter().any(|i| i.key == *key))
        });
        for key in d
            .derived
            .rows
            .iter()
            .flat_map(|r| r.identities.iter().map(|i| &i.key))
        {
            if !self.row_focus.contains_key(key) {
                self.row_focus.insert(key.clone(), cx.focus_handle());
            }
        }
        self.deliver_focus(&d, window, cx);

        let wide = self.wide.get();
        let measured = self.wide.clone();
        let weak = cx.weak_entity();
        // The width this section has; crossing `WIDE_AT` redraws it.
        let probe = canvas(
            move |bounds, window, cx| {
                let now = bounds.size.width >= px(WIDE_AT);
                if measured.get() != now {
                    measured.set(now);
                    window.defer(cx, move |_, cx| {
                        let _ = weak.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let menu = self.menu.as_ref().map(|open| {
            menu_popover(&open.menu, Anchor::Slot(open.anchor.clone()), cx)
                .side(Side::Bottom)
                .align(Align::End)
        });
        let people = self.people_section(&d, cx);
        let side = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.suggestions_card(&d, cx))
            .child(self.unmapped_card(&d, cx));
        root.child(probe)
            .child(
                div()
                    .map(|c| {
                        if wide {
                            c.grid().grid_cols(2).items_start()
                        } else {
                            c.flex().flex_col()
                        }
                    })
                    .gap_4()
                    .child(people)
                    .child(side),
            )
            .children(menu)
    }
}
