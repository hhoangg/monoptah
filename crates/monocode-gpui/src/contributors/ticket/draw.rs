//! Drawing the Ticket tab: the toolbar, the ClickUp problems, the Scope panel,
//! the score card and the leaderboard (`TicketContributorsView.tsx`). The
//! tickets of an expanded member are in `rows.rs`.
use super::{
    backend::Named,
    delivered_line, failed_note, fmt, job_counts, job_fraction, job_label, live, period_phrase,
    plan_line,
    rows::{NoteField, OpenLevel},
    scope::{
        all_excluded, all_spaces, delivered_checked, is_excluded, listed_statuses, norm,
        restore_member, toggle_delivered, toggle_excluded, toggle_space,
    },
    state::{BodyState, Spaces, TicketView, WorkspacesState},
};
use crate::{
    contributors::ui::{self, Dropdown},
    input::{self, Input},
    popover::AnchorSlot,
    settings::widgets::secondary_button,
    theme::*,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Render, Role, SharedString,
    Subscription, Window, canvas, div, prelude::*, px, relative, rgb,
};
use monocode_core::contributors::{
    model::period::PeriodKind,
    tickets::{
        board::{Board, MemberRow, excluded_members_of},
        tickets::{LEVELS, WorkspaceScope},
    },
};
use std::{cell::Cell, collections::HashMap, collections::HashSet, rc::Rc};

/// The Scope panel lays its four lists side by side from this width (`@min-[560px]/ticket`).
const WIDE_AT: f32 = 560.;
const SLIDER_WIDTH: f32 = 176.;
const THUMB: f32 = 12.;

/// The Period segmented control: month, quarter, year, all (no 7d).
pub(super) const KINDS: [(&str, &str); 4] = [
    ("month", "Month"),
    ("quarter", "Quarter"),
    ("year", "Year"),
    ("all", "All"),
];

/// The coverage number field: typed text stays as typed until blur or Enter.
pub(super) struct CoverageField {
    pub input: Entity<Input>,
    pub editing: bool,
    _subscriptions: Vec<Subscription>,
}

/// What the tab remembers between frames besides the rules in `state.rs`.
pub(super) struct UiState {
    pub focus: FocusHandle,
    pub workspace_dd: Dropdown,
    pub coverage: Option<CoverageField>,
    pub slider: AnchorSlot,
    pub slider_focus: FocusHandle,
    pub sliding: bool,
    /// The member whose tickets are open.
    pub open_member: Option<String>,
    /// Tickets ticked for Re-assess selected.
    pub selected: HashSet<String>,
    /// Tickets whose reasons are open.
    pub reasons: HashSet<String>,
    pub notes: HashMap<String, NoteField>,
    pub slots: HashMap<String, AnchorSlot>,
    pub level_menu: Option<OpenLevel>,
    pub wide: Rc<Cell<bool>>,
}
impl UiState {
    pub(super) fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            workspace_dd: Dropdown::new(cx),
            coverage: None,
            slider: AnchorSlot::default(),
            slider_focus: cx.focus_handle(),
            sliding: false,
            open_member: None,
            selected: HashSet::new(),
            reasons: HashSet::new(),
            notes: HashMap::new(),
            slots: HashMap::new(),
            level_menu: None,
            wide: Rc::new(Cell::new(true)),
        }
    }
}

impl UiState {
    /// The tab is opened again: what the user had open is closed, as a fresh mount.
    pub(super) fn reset(&mut self) {
        self.sliding = false;
        self.workspace_dd.open = false;
        self.open_member = None;
        self.selected.clear();
        self.reasons.clear();
        self.notes.clear();
        self.level_menu = None;
        if let Some(field) = &mut self.coverage {
            field.editing = false;
        }
    }
}

impl Focusable for TicketView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.ui.focus.clone()
    }
}

/// The Workspace select's options: a prompt when none is chosen, the token's
/// workspaces, and a saved workspace the token no longer lists (it stays
/// selectable as the current value).
pub(super) fn workspace_options(
    scope: Option<&WorkspaceScope>,
    workspaces: &[Named],
) -> Vec<(String, String)> {
    let mut options = Vec::new();
    if scope.is_none() {
        options.push((String::new(), "Pick a workspace".to_string()));
    }
    options.extend(workspaces.iter().map(|w| (w.id.clone(), w.name.clone())));
    if let Some(scope) = scope
        && !workspaces.iter().any(|w| w.id == scope.workspace_id)
    {
        let label = if scope.name.is_empty() {
            scope.workspace_id.clone()
        } else {
            scope.name.clone()
        };
        options.push((scope.workspace_id.clone(), label));
    }
    options
}

/// The kind the segmented control shows: 7d is not offered.
pub(super) fn shown_kind(kind: PeriodKind) -> PeriodKind {
    if kind == PeriodKind::Days7 {
        PeriodKind::Month
    } else {
        kind
    }
}

impl Render for TicketView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fields(window, cx);
        let workspace_list = self.ui.workspace_dd.open.then(|| {
            let options = workspace_options(self.settings.scope.as_ref(), &self.workspaces);
            let value = self
                .settings
                .scope
                .as_ref()
                .map(|s| s.workspace_id.clone())
                .unwrap_or_default();
            ui::select_list(
                "workspace",
                "Workspace",
                &value,
                options
                    .into_iter()
                    .map(|(id, label)| (id, SharedString::from(label)))
                    .collect(),
                &self.ui.workspace_dd,
                cx,
                |this: &mut Self| &mut this.ui.workspace_dd,
                self.ui.focus.clone(),
                |this, id, _, cx| this.pick_workspace(&id, cx),
            )
        });
        let level_menu = self.ui.level_menu.as_ref().map(|open| open.popover(cx));
        let mut column = vec![self.toolbar(cx)];
        if self.exported {
            column.push(
                div()
                    .id("ticket-exported")
                    .debug_selector(|| "ticket-exported".into())
                    .role(Role::Status)
                    .aria_label("Report saved.")
                    .mb_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .px(px(16.))
                    .py(px(10.))
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .child("Report saved.")
                    .into_any_element(),
            );
        }
        if let Some(error) = self.error() {
            column.push(self.error_line("ticket-error", error));
        }
        if let Some(error) = &self.export_error {
            column.push(self.error_line("ticket-export-error", error));
        }
        column.extend(self.body(cx));
        let measured = self.ui.wide.clone();
        let weak = cx.weak_entity();
        // Measures the width the tab has; a change of side of `WIDE_AT` redraws it.
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
        div()
            .id("contributors-ticket")
            .debug_selector(|| "contributors-body-ticket".into())
            .role(Role::Region)
            .aria_label("Ticket Contributors")
            .track_focus(&self.ui.focus)
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.ui.sliding {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.slide(event.position.x, cx);
                    } else {
                        this.ui.sliding = false;
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.ui.sliding = false),
            )
            .child(
                div()
                    .id("ticket-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .relative()
                            .mx_auto()
                            .w_full()
                            .max_w(px(1024.))
                            .px(px(32.))
                            .pt(px(24.))
                            .pb(px(64.))
                            .child(probe)
                            .children(column),
                    ),
            )
            .children(workspace_list)
            .children(level_menu)
    }
}

impl TicketView {
    // ---- plumbing --------------------------------------------------------------

    /// Makes the inputs the frame needs: the coverage number, and the note of
    /// each ticket of the member that is open.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_coverage(window, cx);
        self.sync_notes(window, cx);
    }

    fn sync_coverage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = fmt(self.settings.percent);
        if self.ui.coverage.is_none() {
            let input = cx.new(|cx| {
                let mut input = Input::field("Score coverage", "", content(0.35), cx);
                input.set_text(text.clone(), cx);
                input.set_max_chars(6);
                input
            });
            let focus = input.read(cx).focus_handle(cx);
            let subscriptions = vec![
                // Enter commits and leaves the field.
                cx.subscribe_in(
                    &input,
                    window,
                    |this, _, _: &input::Submitted, window, cx| {
                        window.focus(&this.ui.focus, cx);
                    },
                ),
                cx.on_focus(&focus, window, |this, _, _| {
                    if let Some(field) = &mut this.ui.coverage {
                        field.editing = true;
                    }
                }),
                cx.on_blur(&focus, window, |this, _, cx| this.commit_coverage(cx)),
            ];
            self.ui.coverage = Some(CoverageField {
                input,
                editing: false,
                _subscriptions: subscriptions,
            });
            return;
        }
        let Some(field) = &self.ui.coverage else {
            return;
        };
        // Typed text stays as typed until blur or Enter; an idle field shows the saved value.
        if !field.editing && field.input.read(cx).content.as_ref() != text {
            field.input.update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    /// Blur or Enter: a typed number is clamped to 0..100 and saved; an
    /// empty or unreadable field snaps back.
    pub(super) fn commit_coverage(&mut self, cx: &mut Context<Self>) {
        let Some(field) = &mut self.ui.coverage else {
            return;
        };
        field.editing = false;
        let input = field.input.clone();
        let draft = input.read(cx).content.to_string();
        let typed = draft.trim().parse::<f64>().ok().filter(|n| n.is_finite());
        match typed {
            Some(typed) => self.set_percent(typed.round().clamp(0., 100.), cx),
            None => {
                let text = fmt(self.settings.percent);
                input.update(cx, |input, cx| input.set_text(text, cx));
            }
        }
    }

    fn slide(&mut self, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.ui.slider.get() else {
            return;
        };
        // A range input's thumb travels the track less its own width.
        let travel = (bounds.size.width.as_f32() - THUMB).max(1.);
        let fraction = ((x - bounds.left()).as_f32() - THUMB / 2.).clamp(0., travel) / travel;
        let value = (100. * f64::from(fraction)).round();
        if self.settings.percent != value {
            self.set_percent(value, cx);
        }
    }

    // ---- the toolbar -----------------------------------------------------------

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let show = self.connected == Some(true) && self.workspaces_state != WorkspacesState::Error;
        let scope = self.settings.scope.as_ref();
        let unreachable = self.unreachable();
        let fetching = self.fetching();
        let kind = shown_kind(self.period.kind);
        let mut bar = div()
            .mb_4()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .debug_selector(|| "ticket-title".into())
                    .mr_auto()
                    .text_size(px(20.))
                    .line_height(px(25.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Ticket Contributors"),
            );
        if !show {
            return bar.into_any_element();
        }
        let options = workspace_options(scope, &self.workspaces);
        let value = scope.map(|s| s.workspace_id.as_str()).unwrap_or("");
        let at = options.iter().position(|(id, _)| id == value).unwrap_or(0);
        let label = options.get(at).map(|(_, l)| l.clone()).unwrap_or_default();
        bar = bar
            .child(ui::select_trigger(
                "workspace",
                "Workspace",
                SharedString::from(label),
                &self.ui.workspace_dd,
                cx,
                |this: &mut Self| &mut this.ui.workspace_dd,
                at,
            ))
            .child(ui::segmented(
                "ticket-period",
                "Period",
                kind.as_str(),
                &KINDS,
                cx,
                |this: &mut Self, value, cx| {
                    if let Some(kind) = PeriodKind::parse(value) {
                        this.set_kind(kind, cx);
                    }
                },
            ));
        if kind != PeriodKind::All {
            let has_next = self.has_next();
            bar = bar.child(
                div()
                    .id("ticket-period-nav")
                    .role(Role::Group)
                    .aria_label("Period navigation")
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .text_size(px(12.))
                    .child(
                        ui::icon_button(
                            "ticket-previous",
                            "Previous period",
                            "chevron-left",
                            14.,
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.go_previous(cx))),
                    )
                    .child(
                        div()
                            .debug_selector(|| "ticket-period-label".into())
                            .min_w(px(72.))
                            .text_center()
                            .child(self.period.label.clone()),
                    )
                    .child(
                        ui::icon_button(
                            "ticket-next",
                            "Next period",
                            "chevron-right",
                            14.,
                            !has_next,
                        )
                        .when(has_next, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| this.go_next(cx)))
                        }),
                    ),
            );
        }
        let refresh_off = self.refresh_disabled();
        let export_off = self.export_disabled();
        bar = bar
            .child(
                ui::icon_button_with(
                    "ticket-refresh",
                    "Refresh from ClickUp",
                    if fetching {
                        crate::right_panel::spinner("ticket-refreshing", "loader", 14.)
                    } else {
                        icon("refresh-cw", 14.)
                            .text_color(content(if refresh_off { 0.35 } else { 0.55 }))
                            .into_any_element()
                    },
                    refresh_off,
                )
                .when(!refresh_off, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                }),
            )
            .child({
                let text = if self.exporting {
                    "Preparing report..."
                } else {
                    "Export report"
                };
                secondary_button("ticket-export", text, false, export_off)
                    .debug_selector(|| "ticket-export".into())
                    .child(text)
                    .when(!export_off, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| this.export_report(cx)))
                    })
            });
        if scope.is_some() && !unreachable {
            let open = self.scope_open;
            bar = bar.child(
                secondary_button("ticket-scope-toggle", "Scope", false, false)
                    .debug_selector(|| "ticket-scope-toggle".into())
                    .aria_expanded(open)
                    .child("Scope")
                    .child(icon(
                        if open {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        },
                        12.,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_scope(cx))),
            );
        }
        bar.into_any_element()
    }

    fn error_line(&self, id: &'static str, text: &str) -> AnyElement {
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .role(Role::Alert)
            .aria_label(text.to_string())
            .mb_4()
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .px(px(16.))
            .py(px(10.))
            .text_size(px(12.))
            .text_color(rgb(ui::RED))
            .child(text.to_string())
            .into_any_element()
    }

    // ---- the body ----------------------------------------------------------------

    fn body(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match self.body_state() {
            BodyState::Problem { message, retry } => vec![self.problem(message, retry, cx)],
            BodyState::Checking => {
                vec![self.status_line("ticket-checking", "Checking ClickUp...")]
            }
            BodyState::LoadingWorkspaces => {
                vec![self.status_line("ticket-loading-workspaces", "Loading workspaces...")]
            }
            BodyState::PickWorkspace => vec![
                div()
                    .id("ticket-pick-workspace")
                    .debug_selector(|| "ticket-pick-workspace".into())
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .px(px(16.))
                    .py(px(32.))
                    .text_center()
                    .text_size(px(13.))
                    .child("Pick a ClickUp workspace.")
                    .into_any_element(),
            ],
            BodyState::Board => {
                let Some(scope) = self.settings.scope.clone() else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                if self.scope_open {
                    out.push(self.scope_panel(&scope, cx));
                }
                out.push(self.score_card(cx));
                out.push(self.leaderboard(cx));
                out
            }
        }
    }

    fn status_line(&self, id: &'static str, text: &'static str) -> AnyElement {
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .role(Role::Status)
            .aria_label(text)
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .text_color(content(0.5))
            .child(crate::right_panel::spinner(
                SharedString::from(format!("{id}-spinner")),
                "loader",
                14.,
            ))
            .child(text)
            .into_any_element()
    }

    /// A message with Open Settings and, when asked, Retry.
    fn problem(&self, message: String, retry: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("ticket-problem")
            .debug_selector(|| "ticket-problem".into())
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .px(px(16.))
            .py(px(32.))
            .text_center()
            .child(div().text_size(px(13.)).child(message))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        secondary_button("ticket-open-settings", "Open Settings", false, false)
                            .debug_selector(|| "ticket-open-settings".into())
                            .child("Open Settings")
                            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                    )
                    .when(retry, |d| {
                        d.child(
                            secondary_button("ticket-retry", "Retry", false, false)
                                .debug_selector(|| "ticket-retry".into())
                                .child("Retry")
                                .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                        )
                    }),
            )
            .into_any_element()
    }

    // ---- the Scope panel ---------------------------------------------------------

    fn list_column(
        &self,
        id: &'static str,
        title: &'static str,
        children: Vec<AnyElement>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .role(Role::Group)
            .aria_label(title)
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(content(0.5))
                    .child(title),
            )
            .children(children)
    }

    fn muted(text: impl Into<SharedString>) -> AnyElement {
        div()
            .text_size(px(12.))
            .text_color(content(0.45))
            .child(text.into())
            .into_any_element()
    }

    fn scope_panel(&self, scope: &WorkspaceScope, cx: &mut Context<Self>) -> AnyElement {
        let wide = self.ui.wide.get();
        let spaces_list = {
            let mut rows = vec![
                ui::checkbox(
                    "ticket-space-all".into(),
                    "All spaces".into(),
                    None,
                    scope.space_ids.is_none(),
                    scope.space_ids.is_none(),
                    cx,
                    |this: &mut Self, _, cx| {
                        if let Some(scope) = this.settings.scope.clone() {
                            this.set_scope(Some(all_spaces(&scope)), cx);
                        }
                    },
                )
                .into_any_element(),
            ];
            match &self.spaces {
                Some(Spaces::Failed(error)) => rows.push(
                    div()
                        .id("ticket-spaces-error")
                        .debug_selector(|| "ticket-spaces-error".into())
                        .role(Role::Alert)
                        .text_size(px(12.))
                        .text_color(rgb(ui::RED))
                        .child(format!("Could not load the spaces: {error}"))
                        .into_any_element(),
                ),
                None | Some(Spaces::Loading) => rows.push(
                    div()
                        .id("ticket-spaces-loading")
                        .debug_selector(|| "ticket-spaces-loading".into())
                        .role(Role::Status)
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child(crate::right_panel::spinner(
                            "ticket-spaces-spinner",
                            "loader",
                            12.,
                        ))
                        .child("Loading spaces...")
                        .into_any_element(),
                ),
                Some(Spaces::Ready(list)) if list.is_empty() => rows.push(Self::muted(
                    "This token cannot list spaces. Tickets from every space are included.",
                )),
                Some(Spaces::Ready(list)) => {
                    for space in list {
                        let id = space.id.clone();
                        rows.push(
                            ui::checkbox(
                                format!("ticket-space-{}", space.id),
                                space.name.clone().into(),
                                None,
                                scope
                                    .space_ids
                                    .as_ref()
                                    .is_some_and(|ids| ids.contains(&space.id)),
                                false,
                                cx,
                                move |this: &mut Self, on, cx| {
                                    if let Some(scope) = this.settings.scope.clone() {
                                        this.set_scope(Some(toggle_space(&scope, &id, on)), cx);
                                    }
                                },
                            )
                            .into_any_element(),
                        );
                    }
                }
            }
            self.list_column("ticket-scope-spaces", "Spaces", rows)
        };

        let delivered = listed_statuses(&self.derived.statuses, &scope.delivered_statuses);
        let delivered_list = self.list_column(
            "ticket-scope-delivered",
            "Delivered statuses",
            std::iter::once(Self::muted(
                "Tickets in these statuses count as delivered work.",
            ))
            .chain(if delivered.is_empty() {
                vec![Self::muted(
                    "Statuses show up here once tickets are fetched.",
                )]
            } else {
                delivered
                    .iter()
                    .map(|status| {
                        let target = status.clone();
                        ui::checkbox(
                            format!("ticket-delivered-{}", norm(&status.name)),
                            status.name.clone().into(),
                            Some(status.status_type.clone().into())
                                .filter(|t: &SharedString| !t.is_empty()),
                            delivered_checked(scope, status),
                            false,
                            cx,
                            move |this: &mut Self, on, cx| {
                                if let Some(scope) = this.settings.scope.clone() {
                                    this.set_scope(Some(toggle_delivered(&scope, &target, on)), cx);
                                }
                            },
                        )
                        .into_any_element()
                    })
                    .collect()
            })
            .collect(),
        );

        let excluded = listed_statuses(&self.derived.statuses, &scope.excluded_statuses);
        let excluded_list = self.list_column(
            "ticket-scope-excluded",
            "Excluded statuses",
            if excluded.is_empty() {
                vec![Self::muted(
                    "Statuses show up here once tickets are fetched.",
                )]
            } else {
                excluded
                    .iter()
                    .map(|status| {
                        let name = status.name.clone();
                        ui::checkbox(
                            format!("ticket-excluded-{}", norm(&status.name)),
                            status.name.clone().into(),
                            None,
                            is_excluded(scope, &status.name),
                            false,
                            cx,
                            move |this: &mut Self, on, cx| {
                                if let Some(scope) = this.settings.scope.clone() {
                                    this.set_scope(Some(toggle_excluded(&scope, &name, on)), cx);
                                }
                            },
                        )
                        .into_any_element()
                    })
                    .collect()
            },
        );

        let members = excluded_members_of(&self.tickets(), scope).into_owned();
        let members_list = self.list_column(
            "ticket-scope-members",
            "Excluded members",
            if members.is_empty() {
                vec![Self::muted(
                    "Nobody is excluded. Use Exclude on a member of the board.",
                )]
            } else {
                members
                    .into_iter()
                    .map(|member| {
                        let id = member.id.clone();
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .text_color(content(0.8))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .child(member.name.clone()),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("ticket-restore-{}", member.id)))
                                    .debug_selector({
                                        let id = member.id.clone();
                                        move || format!("ticket-restore-{id}")
                                    })
                                    .role(Role::Button)
                                    .aria_label(format!("Restore {} to the board", member.name))
                                    .flex_shrink_0()
                                    .rounded(px(4.))
                                    .px(px(6.))
                                    .py(px(2.))
                                    .text_color(content(0.5))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                                    .child("Restore")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(scope) = this.settings.scope.clone() {
                                            this.set_scope(Some(restore_member(&scope, &id)), cx);
                                        }
                                    })),
                            )
                            .into_any_element()
                    })
                    .collect()
            },
        );

        div()
            .id("ticket-scope")
            .debug_selector(|| "ticket-scope".into())
            .role(Role::Group)
            .aria_label("Scope")
            .mb_4()
            .flex()
            .gap(px(32.))
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .px_4()
            .py_3()
            .map(|d| {
                if wide {
                    d.flex_row()
                } else {
                    d.flex_col().gap_3()
                }
            })
            .child(spaces_list)
            .child(delivered_list)
            .child(excluded_list)
            .child(members_list)
            .into_any_element()
    }

    // ---- the score card -------------------------------------------------------------

    fn score_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let board = &self.derived.board;
        let job = self.job.as_ref();
        let busy = live(job);
        let phrase = period_phrase(self.period.kind == PeriodKind::All, &self.period.label);
        let every_status_excluded =
            all_excluded(&self.derived.statuses, self.settings.scope.as_ref());
        let failed = match job {
            Some(job) if job.kind == "assess" && !busy && job.failed > 0 => job.failed,
            _ => 0,
        };
        let score_off = self.derived.to_assess == 0 || busy;
        let note = |id: &'static str, text: String| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .border_t_1()
                .border_color(content(0.1))
                .px(px(16.))
                .py(px(10.))
                .text_size(px(12.))
                .text_color(ui::amber(1.))
                .child(text)
                .into_any_element()
        };
        let coverage = self.ui.coverage.as_ref().map(|field| {
            div()
                .debug_selector(|| "ticket-coverage".into())
                .w(px(64.))
                .h(px(28.))
                .flex()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.05))
                .px(px(8.))
                .text_size(px(12.))
                // Escape leaves the field (and commits it) without closing the page.
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        window.focus(&this.ui.focus, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(field.input.clone())
        });
        div()
            .id("ticket-score-card")
            .debug_selector(|| "ticket-score-card".into())
            .role(Role::Region)
            .aria_label("Score tickets")
            .mb_4()
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .text_size(px(13.))
                    .child(
                        div()
                            .debug_selector(|| "ticket-score-summary".into())
                            .child(delivered_line(board.total, board.assessed, &phrase)),
                    )
                    .when(every_status_excluded, |d| {
                        d.child(
                            div()
                                .debug_selector(|| "ticket-all-excluded".into())
                                .text_size(px(12.))
                                .text_color(ui::amber(1.))
                                .child("Every status is excluded, so no ticket counts."),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_x(px(12.))
                            .gap_y_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(content(0.6))
                                    .child("Score coverage"),
                            )
                            .child(self.slider(cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(12.))
                                    .children(coverage)
                                    .child("%"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_x(px(12.))
                            .gap_y_2()
                            .child(
                                div()
                                    .debug_selector(|| "ticket-plan".into())
                                    .mr_auto()
                                    .text_size(px(12.))
                                    .text_color(content(0.6))
                                    .child(plan_line(
                                        self.derived.to_assess,
                                        self.ai.as_ref(),
                                        self.estimate.as_ref(),
                                    )),
                            )
                            .child(
                                secondary_button("ticket-score", "Score", false, score_off)
                                    .debug_selector(|| "ticket-score".into())
                                    .child("Score")
                                    .when(!score_off, |d| {
                                        d.on_click(cx.listener(|this, _, _, cx| this.score(cx)))
                                    }),
                            ),
                    ),
            )
            .children(job.filter(|_| busy).map(|job| self.job_progress(job, cx)))
            .when(failed > 0, |d| {
                d.child(note("ticket-failed-note", failed_note(failed)))
            })
            .when(board.dates_approximate, |d| {
                d.child(note(
                    "ticket-approximate",
                    "Some delivery dates are approximate (last update). Turn on the Total time in Status ClickApp in ClickUp for exact dates.".into(),
                ))
            })
            .into_any_element()
    }

    fn job_progress(
        &self,
        job: &monocode_core::contributors::ticket_jobs::TicketJobEvent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("ticket-progress-row")
            .debug_selector(|| "ticket-progress-row".into())
            .border_t_1()
            .border_color(content(0.1))
            .px_4()
            .py_3()
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(12.))
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(content(0.6))
                    .child(job_label(job)),
            )
            .child(
                div()
                    .id("ticket-progress")
                    .debug_selector(|| "ticket-progress".into())
                    .role(Role::ProgressIndicator)
                    .aria_label("Ticket job progress")
                    .h(px(6.))
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .rounded_full()
                    .bg(content(0.1))
                    .child(
                        div()
                            .h_full()
                            .w(relative(job_fraction(job)))
                            .bg(accent_color()),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "ticket-progress-counts".into())
                    .flex_shrink_0()
                    .text_color(content(0.6))
                    .child(job_counts(job)),
            )
            .child(
                secondary_button("ticket-cancel", "Cancel", false, false)
                    .debug_selector(|| "ticket-cancel".into())
                    .child("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
            )
            .into_any_element()
    }

    fn slider(&self, cx: &mut Context<Self>) -> AnyElement {
        let fraction = (self.settings.percent / 100.).clamp(0., 1.) as f32;
        div()
            .id("ticket-slider")
            .debug_selector(|| "ticket-slider".into())
            .role(Role::Slider)
            .aria_label("Score coverage slider")
            .track_focus(&self.ui.slider_focus)
            .w(px(SLIDER_WIDTH))
            .max_w_full()
            .h(px(16.))
            .relative()
            .flex()
            .items_center()
            .cursor_pointer()
            .child(self.ui.slider.layout_probe())
            .child(
                div()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .bg(content(0.15))
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .rounded_full()
                            .bg(accent_color()),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px(2.))
                    .left(relative(fraction))
                    .ml(px(-THUMB / 2. + THUMB * (0.5 - fraction)))
                    .size(px(THUMB))
                    .rounded_full()
                    .bg(content(1.)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.ui.sliding = true;
                    window.focus(&this.ui.slider_focus, cx);
                    this.slide(event.position.x, cx);
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let at = this.settings.percent;
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => at - 1.,
                    "right" | "up" => at + 1.,
                    "home" => 0.,
                    "end" => 100.,
                    _ => return,
                };
                this.set_percent(next, cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    // ---- the leaderboard ---------------------------------------------------------------

    fn leaderboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let board = &self.derived.board;
        if board.members.is_empty() && board.unassigned.tickets == 0 {
            return empty_board();
        }
        div()
            .id("ticket-leaderboard")
            .debug_selector(|| "ticket-leaderboard".into())
            .role(Role::Region)
            .aria_label("Leaderboard")
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .child(
                div()
                    .id("ticket-leaderboard-scroll")
                    .role(Role::Region)
                    .aria_label("Leaderboard table")
                    .overflow_x_scroll()
                    .child(
                        div()
                            .id("ticket-leaderboard-table")
                            .role(Role::Table)
                            .min_w(px(TABLE_MIN))
                            .text_size(px(13.))
                            .child(board_head())
                            .children(
                                board
                                    .members
                                    .iter()
                                    .enumerate()
                                    .map(|(index, row)| self.member_row(index, row, cx)),
                            )
                            .when(board.unassigned.tickets > 0, |d| {
                                d.child(unassigned_row(board))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn member_row(&self, index: usize, row: &MemberRow, cx: &mut Context<Self>) -> AnyElement {
        let expanded = self.ui.open_member.as_deref() == Some(row.id.as_str());
        let (toggle_id, exclude_id) = (row.id.clone(), row.id.clone());
        let (exclude_name, selector_id) = (row.name.clone(), row.id.clone());
        let line = div()
            .id(SharedString::from(format!("ticket-board-row-{}", row.id)))
            .flex()
            .border_t_1()
            .border_color(content(0.1))
            .hover(|s| s.bg(content(0.03)))
            .child(
                cell(W_RANK)
                    .text_color(content(0.6))
                    .font_features(crate::right_panel::tabular())
                    .child((index + 1).to_string()),
            )
            .child(
                cell_flex().child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id(SharedString::from(format!("ticket-member-{}", row.id)))
                                .debug_selector({
                                    let id = selector_id.clone();
                                    move || format!("ticket-member-{id}")
                                })
                                .role(Role::Button)
                                .aria_label(row.name.clone())
                                .aria_expanded(expanded)
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .cursor_pointer()
                                .hover(|s| s.text_color(accent_color()))
                                .child(icon("chevron-right", 12.).text_color(content(0.45)).when(
                                    expanded,
                                    |i| {
                                        i.with_transformation(gpui::Transformation::rotate(
                                            gpui::percentage(0.25),
                                        ))
                                    },
                                ))
                                .child(div().min_w_0().truncate().child(row.name.clone()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_member(&toggle_id, cx)
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("ticket-exclude-{}", row.id)))
                                .debug_selector(move || format!("ticket-exclude-{selector_id}"))
                                .role(Role::Button)
                                .aria_label(format!("Exclude {} from the board", row.name))
                                .flex_shrink_0()
                                .rounded(px(4.))
                                .px(px(6.))
                                .py(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.08)))
                                .child(icon("x", 12.).text_color(content(0.5)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.exclude_member(&exclude_id, &exclude_name, cx)
                                })),
                        ),
                ),
            )
            .child(num_cell(W_POINTS, fmt(row.points), false))
            .child(num_cell(W_TICKETS, fmt(row.tickets as f64), false))
            .child(num_cell(
                W_COVERAGE,
                format!("{}/{}", fmt(row.assessed as f64), fmt(row.tickets as f64)),
                false,
            ))
            .child(num_cell(W_ACTIVE, fmt(row.active as f64), row.active == 0))
            .children(LEVELS.iter().map(|level| {
                let count = row.levels[*level];
                num_cell(W_LEVEL, fmt(count as f64), count == 0)
            }));
        div()
            .child(line)
            .when(expanded, |d| d.child(self.member_tickets(row, cx)))
            .into_any_element()
    }

    pub(super) fn toggle_member(&mut self, id: &str, cx: &mut Context<Self>) {
        let open = self.ui.open_member.as_deref() == Some(id);
        self.ui.open_member = (!open).then(|| id.to_string());
        self.ui.selected.clear();
        self.ui.reasons.clear();
        self.ui.level_menu = None;
        cx.notify();
    }

    /// Takes a member off the board at once: the board is rebuilt from the
    /// loaded tickets, no refetch.
    pub(super) fn exclude_member(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let Some(scope) = self.settings.scope.clone() else {
            return;
        };
        if scope.excluded_members.iter().any(|m| m.id == id) {
            return;
        }
        // The row leaves the board; it must not come back expanded after a restore.
        if self.ui.open_member.as_deref() == Some(id) {
            self.ui.open_member = None;
            self.ui.selected.clear();
            self.ui.reasons.clear();
        }
        self.set_scope(Some(super::scope::exclude_member(&scope, id, name)), cx);
    }
}

// ---- table pieces ---------------------------------------------------------------------

const TABLE_MIN: f32 = 760.;
const W_RANK: f32 = 48.;
const W_POINTS: f32 = 80.;
const W_TICKETS: f32 = 72.;
const W_COVERAGE: f32 = 88.;
const W_ACTIVE: f32 = 64.;
const W_LEVEL: f32 = 48.;

pub(super) fn cell(width: f32) -> gpui::Div {
    div()
        .w(px(width))
        .flex_shrink_0()
        .px(px(12.))
        .py(px(8.))
        .flex()
        .items_center()
}

fn cell_flex() -> gpui::Div {
    div()
        .min_w(px(160.))
        .flex_1()
        .px(px(12.))
        .py(px(8.))
        .flex()
        .items_center()
}

fn num_cell(width: f32, text: String, faint: bool) -> gpui::Div {
    cell(width)
        .justify_end()
        .font_features(crate::right_panel::tabular())
        .text_color(content(if faint { 0.3 } else { 1. }))
        .child(text)
}

fn head(width: Option<f32>, text: &'static str, right: bool) -> gpui::Div {
    let base = match width {
        Some(width) => cell(width),
        None => cell_flex(),
    };
    base.when(right, |d| d.justify_end())
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(content(0.5))
        .child(text)
}

fn board_head() -> AnyElement {
    div()
        .flex()
        .child(head(Some(W_RANK), "Rank", false))
        .child(head(None, "Member", false))
        .child(head(Some(W_POINTS), "Points", true))
        .child(head(Some(W_TICKETS), "Tickets", true))
        .child(head(Some(W_COVERAGE), "Coverage", true))
        .child(
            head(Some(W_ACTIVE), "Active", true)
                .id("ticket-active-head")
                .tooltip(|_, cx| {
                    cx.new(|_| {
                        ui::Tip("In a working status during this period. Not scored.".into())
                    })
                    .into()
                }),
        )
        .children(
            LEVELS
                .iter()
                .map(|level| head(Some(W_LEVEL), level.as_str(), true)),
        )
        .into_any_element()
}

fn unassigned_row(board: &Board) -> AnyElement {
    let dash = || num_cell(W_POINTS, "-".into(), false);
    div()
        .id("ticket-unassigned")
        .debug_selector(|| "ticket-unassigned".into())
        .flex()
        .border_t_1()
        .border_color(content(0.1))
        .text_color(content(0.6))
        .child(cell(W_RANK).child("-"))
        .child(cell_flex().child("Unassigned"))
        .child(dash())
        .child(num_cell(
            W_TICKETS,
            fmt(board.unassigned.tickets as f64),
            false,
        ))
        .child(num_cell(
            W_COVERAGE,
            format!(
                "{}/{}",
                fmt(board.unassigned.assessed as f64),
                fmt(board.unassigned.tickets as f64)
            ),
            false,
        ))
        .child(num_cell(W_ACTIVE, "-".into(), false))
        .into_any_element()
}

fn empty_board() -> AnyElement {
    div()
        .id("ticket-empty")
        .debug_selector(|| "ticket-empty".into())
        .rounded_xl()
        .border_1()
        .border_color(content(0.1))
        .px(px(16.))
        .py(px(32.))
        .text_center()
        .text_size(px(12.))
        .text_color(content(0.5))
        .child("No delivered tickets in this period.")
        .into_any_element()
}
