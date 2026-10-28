//! The tickets of an expanded member (`MemberTickets` and `TicketRow` of
//! `TicketContributorsView.tsx`): the select box, the ClickUp link, the level,
//! the points share, the five criteria with their reasons, the manual level
//! and its note, and Re-assess selected.
use super::{draw::cell, fmt, live, state::TicketView};
use crate::{
    contributors::ui,
    input::{self, Input},
    popover::{
        Align, Anchor, AnchorSlot, Menu, MenuAction, MenuEvent, Popover, Side, menu_popover,
    },
    settings::widgets::secondary_button,
    theme::*,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, Focusable, KeyDownEvent, Role,
    SharedString, Stateful, Subscription, Window, div, prelude::*, px,
};
use monocode_core::contributors::{
    model::locale_cmp,
    tickets::{
        board::{MemberRow, MemberTicket},
        tickets::{CRITERIA, LEVELS, delivered_at},
    },
};

const W_SELECT: f32 = 36.;
const W_LEVEL: f32 = 110.;
const W_POINTS: f32 = 100.;
const W_CRITERION: f32 = 92.;
const W_SET: f32 = 120.;
const W_NOTE: f32 = 200.;
const TABLE_MIN: f32 = 1240.;

const CRITERION_LABEL: [&str; 5] = [
    "Ambiguity",
    "Investigation",
    "Scope",
    "Risk",
    "Coordination",
];
const AI_LEVEL: &str = "ai";

/// The note of one ticket: typed text stays as typed until blur or Enter.
pub(super) struct NoteField {
    pub input: Entity<Input>,
    /// The note the ticket carried when the field last followed it.
    pub seen: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// The open "Set level" menu of one ticket.
pub(super) struct OpenLevel {
    pub menu: Entity<Menu>,
    pub anchor: AnchorSlot,
    pub ticket: String,
    _events: Subscription,
}
impl OpenLevel {
    pub(super) fn popover(&self, cx: &App) -> Popover {
        menu_popover(&self.menu, Anchor::Slot(self.anchor.clone()), cx)
            .side(Side::Bottom)
            .align(Align::End)
    }
}

/// The tickets of a member, latest delivery first.
pub(super) fn sorted_entries(row: &MemberRow) -> Vec<&MemberTicket> {
    let mut entries: Vec<&MemberTicket> = row.ticket_list.iter().collect();
    entries.sort_by(|a, b| locale_cmp(delivered_at(&b.ticket).at, delivered_at(&a.ticket).at));
    entries
}

/// The note text a field commits: trimmed, none when blank.
pub(super) fn note_value(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

impl TicketView {
    /// Makes the note field and level anchor of each ticket of the open member.
    pub(super) fn sync_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.ui.open_member.clone() else {
            return;
        };
        let tickets: Vec<(String, Option<String>, bool)> = self
            .derived
            .board
            .members
            .iter()
            .find(|member| member.id == open)
            .map(|row| {
                row.ticket_list
                    .iter()
                    .map(|entry| {
                        (
                            entry.ticket.id.clone(),
                            entry.ticket.override_note.clone(),
                            entry.ticket.override_level.is_some(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (id, note, edited) in tickets {
            self.ui.slots.entry(id.clone()).or_default();
            let placeholder = if edited {
                "Why this level"
            } else {
                "Set a level to add a note"
            };
            if let Some(field) = self.ui.notes.get_mut(&id) {
                field.input.update(cx, |input, cx| {
                    input.set_read_only(!edited);
                    input.set_placeholder(placeholder, cx);
                });
                if field.seen != note {
                    field.seen = note.clone();
                    let text = note.unwrap_or_default();
                    field.input.update(cx, |input, cx| input.set_text(text, cx));
                }
                continue;
            }
            let title = self
                .derived
                .shown
                .iter()
                .find(|ticket| ticket.id == id)
                .map(|ticket| ticket.title.clone())
                .unwrap_or_default();
            let input = cx.new(|cx| {
                let mut input =
                    Input::field(format!("Note for {title}"), placeholder, content(0.35), cx);
                input.set_read_only(!edited);
                input.set_text(note.clone().unwrap_or_default(), cx);
                input
            });
            let focus = input.read(cx).focus_handle(cx);
            let ticket = id.clone();
            let subscriptions = vec![
                // Enter leaves the field, and leaving commits.
                cx.subscribe_in(
                    &input,
                    window,
                    |this, _, _: &input::Submitted, window, cx| {
                        window.focus(&this.ui.focus, cx);
                    },
                ),
                cx.on_blur(&focus, window, move |this, _, cx| {
                    this.commit_note(&ticket, cx)
                }),
            ];
            self.ui.notes.insert(
                id,
                NoteField {
                    input,
                    seen: note,
                    _subscriptions: subscriptions,
                },
            );
        }
    }

    fn ticket_of(
        &self,
        id: &str,
    ) -> Option<&monocode_core::contributors::tickets::tickets::TicketRecord> {
        self.derived.shown.iter().find(|ticket| ticket.id == id)
    }

    /// Blur: a changed note of a ticket with a manual level is saved.
    pub(super) fn commit_note(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(field) = self.ui.notes.get(id) else {
            return;
        };
        let next = note_value(field.input.read(cx).content.as_ref());
        let Some(ticket) = self.ticket_of(id) else {
            return;
        };
        if next == ticket.override_note {
            return;
        }
        if let Some(level) = ticket.override_level.clone() {
            self.set_override(id, Some(&level), next.as_deref(), cx);
        }
    }

    /// The Set level menu picked: AI level clears the manual level, a level sets it
    /// with the note typed so far.
    fn picked_level(&mut self, id: &str, choice: &str, cx: &mut Context<Self>) {
        if choice == AI_LEVEL {
            self.set_override(id, None, None, cx);
            return;
        }
        let Some(level) = choice.strip_prefix("level:") else {
            return;
        };
        let note = self
            .ui
            .notes
            .get(id)
            .and_then(|field| note_value(field.input.read(cx).content.as_ref()));
        self.set_override(id, Some(level), note.as_deref(), cx);
    }

    pub(super) fn open_level_menu(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ticket) = self.ticket_of(id) else {
            return;
        };
        let current = ticket.override_level.clone();
        let title = ticket.title.clone();
        let anchor = self.ui.slots.entry(id.to_string()).or_default().clone();
        let mut items: Vec<crate::popover::MenuItem> = vec![
            MenuAction::new(AI_LEVEL, "AI level")
                .checked(current.is_none())
                .into(),
        ];
        items.extend(LEVELS.iter().map(|level| {
            MenuAction::new(format!("level:{}", level.as_str()), level.as_str())
                .checked(current.as_deref() == Some(level.as_str()))
                .into()
        }));
        let menu =
            cx.new(|cx| Menu::new(format!("Level of {title}"), items, window, cx).width(140.));
        let ticket = id.to_string();
        let events = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            this.ui.level_menu = None;
            if let MenuEvent::Picked(choice, _) = event {
                this.picked_level(&ticket, choice, cx);
            }
            cx.notify();
        });
        self.ui.level_menu = Some(OpenLevel {
            menu,
            anchor,
            ticket: id.to_string(),
            _events: events,
        });
        cx.notify();
    }

    /// Re-assess selected: the ticked tickets of the open member.
    pub(super) fn reassess_selected(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.ui.open_member.clone() else {
            return;
        };
        let ids: Vec<String> = self
            .derived
            .board
            .members
            .iter()
            .find(|member| member.id == open)
            .map(|row| {
                sorted_entries(row)
                    .into_iter()
                    .filter(|entry| self.ui.selected.contains(&entry.ticket.id))
                    .map(|entry| entry.ticket.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        self.reassess(ids, cx);
        self.ui.selected.clear();
        cx.notify();
    }

    fn select_ticket(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            self.ui.selected.insert(id.to_string());
        } else {
            self.ui.selected.remove(id);
        }
        cx.notify();
    }

    fn toggle_reasons(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.ui.reasons.remove(id) {
            self.ui.reasons.insert(id.to_string());
        }
        cx.notify();
    }

    // ---- drawing -----------------------------------------------------------

    pub(super) fn member_tickets(&self, row: &MemberRow, cx: &mut Context<Self>) -> AnyElement {
        let entries = sorted_entries(row);
        let frame = || {
            div()
                .id(SharedString::from(format!("ticket-tickets-{}", row.id)))
                .debug_selector({
                    let id = row.id.clone();
                    move || format!("ticket-tickets-{id}")
                })
                .border_t_1()
                .border_color(content(0.1))
                .bg(content(0.02))
        };
        if entries.is_empty() {
            return frame()
                .px(px(12.))
                .py(px(12.))
                .text_size(px(12.))
                .text_color(content(0.5))
                .child("No delivered tickets in this period.")
                .into_any_element();
        }
        let chosen = entries
            .iter()
            .filter(|entry| self.ui.selected.contains(&entry.ticket.id))
            .count();
        let busy = live(self.job.as_ref());
        let reassess_off = chosen == 0 || busy;
        frame()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .px(px(12.))
                    .py(px(8.))
                    .child(
                        secondary_button(
                            "ticket-reassess",
                            "Re-assess selected",
                            false,
                            reassess_off,
                        )
                        .debug_selector(|| "ticket-reassess".into())
                        .child("Re-assess selected")
                        .when(!reassess_off, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| this.reassess_selected(cx)))
                        }),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("ticket-scroll-{}", row.id)))
                    .role(Role::Region)
                    .aria_label(format!("Tickets of {}", row.name))
                    .overflow_x_scroll()
                    .child(
                        div()
                            .id(SharedString::from(format!("ticket-table-{}", row.id)))
                            .role(Role::Table)
                            .min_w(px(TABLE_MIN))
                            .text_size(px(13.))
                            .child(ticket_head())
                            .children(entries.into_iter().map(|entry| self.ticket_row(entry, cx))),
                    ),
            )
            .into_any_element()
    }

    fn ticket_row(&self, entry: &MemberTicket, cx: &mut Context<Self>) -> AnyElement {
        let ticket = &entry.ticket;
        let id = ticket.id.clone();
        let edited = ticket.override_level.is_some();
        let selected = self.ui.selected.contains(&id);
        let reasons_open = self.ui.reasons.contains(&id);
        let url = ticket.url.clone();
        let line = div()
            .id(SharedString::from(format!("ticket-row-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("ticket-row-{id}")
            })
            .role(Role::Row)
            .flex()
            .border_t_1()
            .border_color(content(0.1))
            .hover(|s| s.bg(content(0.03)))
            .child(cell(W_SELECT).child(select_box(
                format!("ticket-select-{id}"),
                format!("Select {}", ticket.title),
                selected,
                {
                    let id = id.clone();
                    cx.listener(move |this, _, _, cx| this.select_ticket(&id, !selected, cx))
                },
            )))
            .child(
                div()
                    .min_w(px(240.))
                    .flex_1()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(SharedString::from(format!("ticket-link-{id}")))
                            .debug_selector({
                                let id = id.clone();
                                move || format!("ticket-link-{id}")
                            })
                            .role(Role::Link)
                            .aria_label(ticket.title.clone())
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(accent_color())
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                            .child(ticket.title.clone())
                            .on_click(move |_, _, cx| {
                                if url.starts_with("https://") {
                                    cx.open_url(&url);
                                }
                            }),
                    )
                    .child(
                        div().text_size(px(11.)).text_color(content(0.45)).child(
                            ticket
                                .custom_id
                                .clone()
                                .unwrap_or_else(|| format!("#{}", ticket.id)),
                        ),
                    ),
            )
            .child(
                cell(W_LEVEL).whitespace_nowrap().child(match entry.level {
                    Some(level) => div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .rounded_full()
                                .border_1()
                                .border_color(content(0.1))
                                .px(px(6.))
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(level.as_str()),
                        )
                        .when(edited, |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(content(0.5))
                                    .child("edited"),
                            )
                        }),
                    None => div()
                        .text_size(px(11.))
                        .text_color(content(0.45))
                        .child("Not scored"),
                }),
            )
            .child(
                cell(W_POINTS)
                    .justify_end()
                    .whitespace_nowrap()
                    .font_features(crate::right_panel::tabular())
                    .child(if entry.level.is_some() {
                        fmt(entry.share)
                    } else {
                        "-".into()
                    })
                    .when(entry.level.is_some() && entry.share != entry.points, |d| {
                        d.child(
                            div()
                                .ml(px(4.))
                                .text_size(px(11.))
                                .text_color(content(0.45))
                                .child(format!("of {}", fmt(entry.points))),
                        )
                    }),
            )
            .children(CRITERIA.iter().enumerate().map(|(at, criterion)| {
                let toggle = id.clone();
                cell(W_CRITERION)
                    .justify_end()
                    .font_features(crate::right_panel::tabular())
                    .child(match &ticket.assessment {
                        Some(assessment) => {
                            let score = assessment.criteria.get(at);
                            div()
                                .id(SharedString::from(format!(
                                    "ticket-criterion-{id}-{criterion}"
                                )))
                                .debug_selector({
                                    let id = id.clone();
                                    let criterion = *criterion;
                                    move || format!("ticket-criterion-{id}-{criterion}")
                                })
                                .role(Role::Button)
                                .aria_label(format!(
                                    "{} {score}, {} reasons",
                                    CRITERION_LABEL[at],
                                    if reasons_open { "hide" } else { "show" }
                                ))
                                .aria_expanded(reasons_open)
                                .rounded(px(2.))
                                .cursor_pointer()
                                .hover(|s| s.text_color(accent_color()))
                                .tooltip({
                                    let reason = assessment.reasons.get(at).to_string();
                                    move |_, cx| cx.new(|_| ui::Tip(reason.clone().into())).into()
                                })
                                .child(score.to_string())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_reasons(&toggle, cx)
                                }))
                                .into_any_element()
                        }
                        None => div().text_color(content(0.3)).child("-").into_any_element(),
                    })
            }))
            .child(cell(W_SET).child(self.level_trigger(entry, cx)))
            .child(cell(W_NOTE).child(self.note_box(&id, cx)));
        let reasons = reasons_open
            .then_some(ticket.assessment.as_ref())
            .flatten()
            .map(|assessment| {
                div()
                    .id(SharedString::from(format!("ticket-reasons-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("ticket-reasons-{id}")
                    })
                    .bg(content(0.03))
                    .pl(px(W_SELECT))
                    .px(px(12.))
                    .pt(px(4.))
                    .pb(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .children(CRITERION_LABEL.iter().enumerate().map(|(at, label)| {
                        let reason = assessment.reasons.get(at);
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(112.))
                                    .flex_shrink_0()
                                    .text_color(content(0.5))
                                    .child(*label),
                            )
                            .child(div().min_w_0().child(if reason.is_empty() {
                                "No reason given.".to_string()
                            } else {
                                reason.to_string()
                            }))
                    }))
            });
        div().child(line).children(reasons).into_any_element()
    }

    fn level_trigger(&self, entry: &MemberTicket, cx: &mut Context<Self>) -> AnyElement {
        let ticket = &entry.ticket;
        let id = ticket.id.clone();
        let open = self
            .ui
            .level_menu
            .as_ref()
            .is_some_and(|menu| menu.ticket == id);
        let slot = self.ui.slots.get(&id).cloned().unwrap_or_default();
        let text = ticket
            .override_level
            .clone()
            .unwrap_or_else(|| "AI level".into());
        div()
            .id(SharedString::from(format!("ticket-level-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("ticket-level-{id}")
            })
            .role(Role::ComboBox)
            .aria_label(format!("Level of {}: {text}", ticket.title))
            .aria_expanded(open)
            .relative()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.05))
            .px_2()
            .py_1()
            .text_size(px(12.))
            .line_height(px(16.))
            .cursor_pointer()
            .hover(|s| s.border_color(content(0.2)))
            .child(slot.probe())
            .child(div().min_w_0().truncate().child(text))
            .child(icon("chevron-down-hi", 14.).text_color(content(0.5)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_level_menu(&id, window, cx);
            }))
            .into_any_element()
    }

    fn note_box(&self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(field) = self.ui.notes.get(id) else {
            return div().into_any_element();
        };
        let edited = self
            .ticket_of(id)
            .is_some_and(|ticket| ticket.override_level.is_some());
        div()
            .debug_selector({
                let id = id.to_string();
                move || format!("ticket-note-{id}")
            })
            .w_full()
            .h(px(28.))
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.05))
            .px(px(8.))
            .text_size(px(12.))
            .when(!edited, |d| d.opacity(0.5))
            // Escape leaves the field (and commits it) without closing the page.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    window.focus(&this.ui.focus, cx);
                    cx.stop_propagation();
                }
            }))
            .child(field.input.clone())
            .into_any_element()
    }
}

fn select_box(
    id: String,
    label: String,
    checked: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let selector = id.clone();
    div()
        .id(SharedString::from(id))
        .debug_selector(move || selector.clone())
        .role(Role::CheckBox)
        .aria_label(label)
        .aria_toggled(if checked {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .size(px(14.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if checked {
            accent_color()
        } else {
            content(0.3)
        })
        .cursor_pointer()
        .when(checked, |d| {
            d.bg(accent_color())
                .child(icon("check", 10.).text_color(gpui::white()))
        })
        .on_click(on_click)
}

fn ticket_head() -> AnyElement {
    let head = |base: Div, text: &'static str, right: bool| {
        base.when(right, |d| d.justify_end())
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(content(0.5))
            .child(text)
    };
    div()
        .flex()
        .child(head(cell(W_SELECT), "", false))
        .child(head(
            div()
                .min_w(px(240.))
                .flex_1()
                .px(px(12.))
                .py(px(8.))
                .flex()
                .items_center(),
            "Ticket",
            false,
        ))
        .child(head(cell(W_LEVEL), "Level", false))
        .child(head(cell(W_POINTS), "Points", true))
        .children(
            CRITERION_LABEL
                .iter()
                .map(|label| head(cell(W_CRITERION), label, true)),
        )
        .child(head(cell(W_SET), "Set level", false))
        .child(head(cell(W_NOTE), "Note", false))
        .into_any_element()
}
