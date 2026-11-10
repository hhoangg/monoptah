//! Sidebar.tsx's inset `RightPanel`: the project tools beside the card, with
//! reorderable Explorer / Changes / History tabs in the title bar, and a
//! trailing Terminal tab when the terminal lives in the panel.
use crate::{
    changes::Changes,
    explorer::Explorer,
    git_status::{GitChanged, ProjectGit},
    history::History,
    panel_root::{Chrome, Link, PanelMode, SETUP_CHANGED, SetupStrip, SetupTone},
    project_tabs::{TabLabel, TabReveal, tab_button},
    theme::*,
};
use gpui::{
    AnyElement, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, FontFeatures,
    FontWeight, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Role, SharedString,
    Stateful, StyleRefinement, Subscription, WeakEntity, Window, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) const WIDTH_MIN: f32 = 220.;
pub(crate) const WIDTH_MAX: f32 = 640.;
pub(crate) const WIDTH_DEFAULT: f32 = 288.;
/// `count` 26px icon tabs, one unfolded label with 4px extra padding, and the gaps.
pub(crate) fn tabs_width(count: usize) -> f32 {
    tab_icons_width(count) + 64. + 4.
}
pub(crate) fn tab_icons_width(count: usize) -> f32 {
    count as f32 * 26. + count.saturating_sub(1) as f32 * 2.
}

pub(crate) fn tab_label_width(width: f32, count: usize) -> f32 {
    // The selected tab adds 2px of padding per side. Below 30px there is
    // insufficient room for a useful abbreviated label and its 6px gap.
    let available = (width - tab_icons_width(count) - 4.).max(0.);
    if available < 30. { 0. } else { available }
}

// Separators inside the inset card use its 4% hairline; dark selection is 10%.
pub(crate) fn stroke() -> Hsla {
    content(0.04)
}
pub(crate) fn selection() -> Hsla {
    content(0.1)
}
pub(crate) const DIFF_ADD: u32 = 0x6ee7b7;
pub(crate) const DIFF_DEL: u32 = 0xfda4af;
// Tailwind v4.
pub(crate) const AMBER_400: u32 = 0xffb900;
pub(crate) const SKY_400: u32 = 0x00bcff;
pub(crate) const RED_400: u32 = 0xff6467;
/// `--color-background-base` in the default dark theme, hsl(240 0% 9%).
pub(crate) const BACKGROUND_BASE: u32 = 0x171717;

pub(crate) fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RightTab {
    #[default]
    Files,
    Changes,
    Graph,
    /// The project's terminal, when it opens in the panel. Always last and
    /// never part of the saved order.
    Terminal,
    /// The startup of a cloud session's codespace. Opened by the shell, never part of the tab
    /// strip or the saved order, and never saved as the panel's tab.
    Codespace,
}
impl RightTab {
    fn label(self) -> &'static str {
        match self {
            Self::Files => "Explorer",
            Self::Changes => "Changes",
            Self::Graph => "History",
            Self::Terminal => "Terminal",
            Self::Codespace => "Codespace",
        }
    }
    /// loadRightPanelTabOrder: unique known tabs, the missing ones appended.
    /// Changes has no tab; the title bar's diff stat opens it.
    pub(crate) fn normalize(order: &[RightTab]) -> Vec<RightTab> {
        let mut out = Vec::new();
        for tab in order.iter().chain(&[Self::Files, Self::Graph]) {
            if !matches!(tab, Self::Terminal | Self::Changes | Self::Codespace)
                && !out.contains(tab)
            {
                out.push(*tab);
            }
        }
        out
    }
}

pub(crate) enum PanelEvent {
    Tab(RightTab),
    Reordered(Vec<RightTab>),
    /// The Cloud / Local switch of a cloud session's panels.
    Mode(PanelMode),
    /// "Update Claude setup": a user action that copies the local setup to the workspace again.
    UpdateSetup,
    /// The connection strip's Connect action (a user action that may wake the workspace).
    Connect,
}

/// Tool headers share the open-file tab bar's height and bottom rule.
pub(crate) fn pane_header(id: &'static str, label: &'static str) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Toolbar)
        .aria_label(label)
        .h(px(PANE_HEADER_HEIGHT))
        .flex_shrink_0()
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(stroke())
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(content(0.85))
}

/// GitChangesPanel's `IconAction`: a 20px square, ink at 55%.
pub(crate) fn icon_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    path: &'static str,
    enabled: bool,
) -> Stateful<gpui::Div> {
    let id = id.into();
    let group: SharedString = format!("icon-action-{id:?}").into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.into())
        .group(group.clone())
        .size(px(20.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .when(!enabled, |d| d.opacity(0.4))
        .when(enabled, |d| d.hover(|s| s.bg(content(0.1))))
        .child(
            icon(path, 14.)
                .text_color(content(0.55))
                .when(enabled, |i| {
                    i.group_hover(group, |s| s.text_color(content(1.)))
                }),
        )
}

/// The `animate-spin` Loader.
pub(crate) fn spinner(id: impl Into<gpui::ElementId>, path: &'static str, side: f32) -> AnyElement {
    use gpui::{Transformation, percentage};
    crate::pulse::looping(
        id,
        std::time::Duration::from_secs(1),
        crate::pulse::Rate::Smooth,
        move |delta| {
            icon(path, side).with_transformation(Transformation::rotate(percentage(delta)))
        },
    )
    .into_any_element()
}

pub(crate) fn format_integer(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 { format!("-{out}") } else { out }
}

pub(crate) fn diff_stat_width(stats: Option<&monocode_core::scm::DiffStats>) -> f32 {
    let added = format_integer(stats.map_or(0, |s| s.additions.max(0)));
    let deleted = format_integer(stats.map_or(0, |s| s.deletions.max(0)));
    // Tabular digits at 10px, the signs, a 6px gap and 6px padding per side.
    (added.len() + deleted.len() + 2) as f32 * 6.5 + 18.
}

/// Fit the complete diff label together so one number cannot push the other
/// outside its slot. Callers include any gap that scales with their text.
pub(crate) fn diff_stat_scale(available: f32, natural: f32) -> f32 {
    (available.max(0.) / natural.max(f32::EPSILON)).clamp(0.1, 1.)
}

pub(crate) fn diff_stat(
    stats: Option<&monocode_core::scm::DiffStats>,
    width: f32,
) -> Stateful<gpui::Div> {
    let added = format_integer(stats.map_or(0, |s| s.additions.max(0)));
    let deleted = format_integer(stats.map_or(0, |s| s.deletions.max(0)));
    let scale = diff_stat_scale(width - 18., diff_stat_width(stats) - 18.);
    div()
        .id("project-diff-stat")
        .debug_selector(|| "project-diff-stat".into())
        .role(Role::Button)
        .aria_label(format!(
            "Open Changes: {added} lines added, {deleted} lines deleted"
        ))
        .w(px(width))
        .h(px(24.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(6.))
        .px_1p5()
        .rounded_md()
        .whitespace_nowrap()
        .text_size(px(10. * scale))
        .font_weight(FontWeight::MEDIUM)
        .font_features(tabular())
        .child(
            div()
                .text_color(gpui::rgb(DIFF_ADD))
                .child(format!("+{added}")),
        )
        .child(
            div()
                .text_color(gpui::rgb(DIFF_DEL))
                .child(format!("-{deleted}")),
        )
}

struct TabDrag {
    tab: RightTab,
    origin: Pixels,
    offset: Pixels,
    active: bool,
}

/// The title-bar strip observes the panel so selection and drag motion
/// repaint independently of the cached pane and session tabs.
pub(crate) struct ProjectTabs {
    panel: WeakEntity<RightPanel>,
    _subscription: Subscription,
}

impl Render for ProjectTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.panel
            .update(cx, |panel, cx| panel.tab_strip(window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// Whether the Git status or the file tree is on screen.
fn status_visible(shown: bool, tab: RightTab) -> bool {
    shown && matches!(tab, RightTab::Files | RightTab::Changes)
}

pub(crate) struct RightPanel {
    git: Entity<ProjectGit>,
    pub tabs: Entity<ProjectTabs>,
    pub explorer: Entity<Explorer>,
    pub changes: Entity<Changes>,
    pub history: Entity<History>,
    pub codespace: Entity<crate::codespace_pane::CodespacePane>,
    focus: FocusHandle,
    tab: RightTab,
    order: Vec<RightTab>,
    shown: bool,
    sidebar_tab: Option<RightTab>,
    /// Whether the Terminal tab follows the others; the shell draws its pane.
    terminal_tab: bool,
    /// Classic reserves the title bar's height; rounded content starts at
    /// the card's top edge, below the window-wide title bar.
    top_inset: f32,
    /// Set while a cloud session is in view: which side the panels show, the switch and the
    /// connection strip.
    cloud: Option<Chrome>,
    /// The Claude setup block shows its server list.
    setup_open: bool,
    /// Name of the Codespace tab: `Codespace` or `Sandbox`, by the backend of the start shown.
    codespace_title: &'static str,
    tab_width: f32,
    drag: Option<TabDrag>,
    reveal: Option<TabReveal>,
    skip_click: bool,
    tab_bounds: Vec<crate::popover::AnchorSlot>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<PanelEvent> for RightPanel {}
impl Focusable for RightPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RightPanel {
    pub(crate) fn new(
        git: Entity<ProjectGit>,
        tab: RightTab,
        order: Vec<RightTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let explorer = cx.new(|cx| Explorer::new(git.clone(), window, cx));
        let changes = cx.new(|cx| Changes::new(git.clone(), window, cx));
        let history = cx.new(|cx| History::new(git.clone(), window, cx));
        let codespace = cx.new(|_| crate::codespace_pane::CodespacePane::new());
        let subscriptions = vec![
            cx.observe(&git, |_, _, cx| cx.notify()),
            cx.subscribe(&git, |_, _, _: &GitChanged, cx| cx.notify()),
        ];
        let panel = cx.entity();
        let tabs = cx.new(|cx| ProjectTabs {
            panel: panel.downgrade(),
            _subscription: cx.observe(&panel, |_, _, cx| cx.notify()),
        });
        let mut panel = Self {
            git,
            tabs,
            focus: cx.focus_handle(),
            explorer,
            changes,
            history,
            codespace,
            tab,
            order: RightTab::normalize(&order),
            shown: false,
            sidebar_tab: None,
            terminal_tab: false,
            top_inset: 0.,
            cloud: None,
            setup_open: false,
            codespace_title: RightTab::Codespace.label(),
            tab_width: tabs_width(3),
            drag: None,
            reveal: None,
            skip_click: false,
            tab_bounds: (0..4).map(|_| Default::default()).collect(),
            _subscriptions: subscriptions,
        };
        panel.sync_panes(cx);
        panel
    }
    pub(crate) fn set_cloud_chrome(&mut self, chrome: Option<Chrome>, cx: &mut Context<Self>) {
        if self.cloud != chrome {
            let browser = chrome.as_ref().is_some_and(|chrome| chrome.browser);
            let pending = chrome.as_ref().is_some_and(|chrome| chrome.placeholder);
            self.cloud = chrome;
            self.explorer.update(cx, |explorer, cx| {
                explorer.set_offers_browser(browser, cx);
                explorer.set_read_only(pending, cx);
            });
            cx.notify();
        }
    }
    pub(crate) fn set_codespace_title(&mut self, title: &'static str, cx: &mut Context<Self>) {
        if self.codespace_title != title {
            self.codespace_title = title;
            cx.notify();
        }
    }
    #[cfg(test)]
    pub(crate) fn codespace_title(&self) -> &'static str {
        self.codespace_title
    }
    #[cfg(test)]
    pub(crate) fn cloud_banner(&self) -> Option<&str> {
        self.cloud.as_ref().map(|chrome| chrome.banner.as_str())
    }
    #[cfg(test)]
    pub(crate) fn cloud_chrome(&self) -> Option<&Chrome> {
        self.cloud.as_ref()
    }
    pub(crate) fn tab(&self) -> RightTab {
        self.tab
    }
    /// The tabs in strip order: the saved two, with Terminal between them
    /// when enabled.
    pub(crate) fn tabs(&self) -> Vec<RightTab> {
        let mut tabs = self.order.clone();
        if self.terminal_tab {
            tabs.insert(1.min(tabs.len()), RightTab::Terminal);
        }
        tabs
    }
    /// Adds or removes the Terminal tab; Explorer takes over when it goes.
    pub(crate) fn set_terminal_tab(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.terminal_tab == enabled {
            return;
        }
        self.terminal_tab = enabled;
        if !enabled && self.tab == RightTab::Terminal {
            self.select(RightTab::Files, cx);
        }
        cx.notify();
    }
    pub(crate) fn set_show_excluded(&mut self, show: bool, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.set_show_excluded(show, cx));
    }
    pub(crate) fn top_inset(&self) -> f32 {
        self.top_inset
    }
    pub(crate) fn set_top_inset(&mut self, height: f32, cx: &mut Context<Self>) {
        if self.top_inset != height {
            self.top_inset = height;
            cx.notify();
        }
    }
    pub(crate) fn set_tab_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if self.tab_width != width {
            self.tab_width = width;
            cx.notify();
        }
    }
    /// The panes read and poll only while their tab is on screen.
    pub(crate) fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        self.sync_panes(cx);
    }
    pub(crate) fn select(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.reveal = Some(TabReveal::new(tab, self.tab));
        }
        self.tab = tab;
        self.sync_panes(cx);
        cx.emit(PanelEvent::Tab(tab));
        cx.notify();
    }
    pub(crate) fn set_sidebar_tab(&mut self, tab: Option<RightTab>, cx: &mut Context<Self>) {
        if self.sidebar_tab != tab {
            self.sidebar_tab = tab;
            self.sync_panes(cx);
        }
    }
    fn sync_panes(&mut self, cx: &mut Context<Self>) {
        let (shown, tab) = match self.sidebar_tab {
            Some(tab) => (true, tab),
            None => (self.shown, self.tab),
        };
        // Cloud workspaces are watched only while Files or Changes is on screen, in the panel or
        // in the classic layout's sidebar.
        let visible = status_visible(shown, tab);
        self.git
            .update(cx, |git, cx| git.set_panel_visible(visible, cx));
        self.explorer.update(cx, |e, cx| {
            e.set_enabled(shown && tab == RightTab::Files, cx)
        });
        self.changes.update(cx, |c, cx| {
            c.set_enabled(shown && tab == RightTab::Changes, cx)
        });
        self.history.update(cx, |h, cx| {
            h.set_enabled(shown && tab == RightTab::Graph, cx)
        });
    }
    pub(crate) fn focus_pane(&self, window: &mut Window, cx: &mut App) {
        let focus = match (self.shown, self.tab) {
            (true, RightTab::Files) => self.explorer.focus_handle(cx),
            (true, RightTab::Graph) => self.history.focus_handle(cx),
            _ => self.focus.clone(),
        };
        window.focus(&focus, cx);
    }
    pub(crate) fn finish_opening(&self, window: &mut Window, cx: &mut App) {
        if self.focus.is_focused(window) {
            self.focus_pane(window, cx);
        }
    }

    fn start_drag(&mut self, tab: RightTab, event: &MouseDownEvent) {
        self.drag = Some(TabDrag {
            tab,
            origin: event.position.x,
            offset: px(0.),
            active: false,
        });
    }
    fn move_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let tabs = self.tabs();
        let Some(drag) = &mut self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_drag(cx);
            return;
        }
        drag.offset = event.position.x - drag.origin;
        if !drag.active && drag.offset.abs() < px(4.) {
            return;
        }
        drag.active = true;
        // Swap with a neighbour once the dragged tab's centre crosses its centre.
        // Terminal holds its slot, outside the reorderable tabs.
        let Some(from) = tabs.iter().position(|t| *t == drag.tab) else {
            return;
        };
        let bounds: Vec<Option<Bounds<Pixels>>> = tabs
            .iter()
            .zip(&self.tab_bounds)
            .map(|(tab, slot)| slot.get().filter(|_| *tab != RightTab::Terminal))
            .collect();
        if let Some(Some(own)) = bounds.get(from) {
            let centre = own.center().x + drag.offset;
            let target = bounds
                .iter()
                .enumerate()
                .filter(|(ix, _)| *ix != from)
                .find(|(ix, b)| {
                    b.is_some_and(|b| {
                        if *ix > from {
                            centre > b.center().x
                        } else {
                            centre < b.center().x
                        }
                    })
                })
                .map(|(ix, _)| ix);
            if let Some(to) = target
                && let Some(Some(other)) = bounds.get(to)
            {
                // A selected tab includes its label, so moving right crosses
                // the other tabs' widths rather than the selected tab's width.
                let shift = if to > from {
                    other.right() - own.right()
                } else {
                    other.left() - own.left()
                };
                let (a, b) = (
                    self.order.iter().position(|t| *t == tabs[from]),
                    self.order.iter().position(|t| *t == tabs[to]),
                );
                if let (Some(a), Some(b)) = (a, b) {
                    self.order.swap(a, b);
                }
                drag.origin += shift;
                drag.offset -= shift;
            }
        }
        cx.notify();
    }
    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take()
            && drag.active
        {
            self.skip_click = true;
            cx.emit(PanelEvent::Reordered(self.order.clone()));
            cx.notify();
        }
    }

    fn tab_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let progress = self
            .reveal
            .as_ref()
            .and_then(|reveal| reveal.progress(self.tab, window, cx));
        let dragging = self
            .drag
            .as_ref()
            .filter(|d| d.active)
            .map(|d| (d.tab, d.offset));
        let count = self.tabs().len();
        let tabs: Vec<AnyElement> = self
            .tabs()
            .into_iter()
            .enumerate()
            .map(|(ix, tab)| {
                let active = self.tab == tab;
                let (id, path) = match tab {
                    RightTab::Files => ("right-panel-tab-Explorer", "file-script"),
                    RightTab::Changes => ("right-panel-tab-Changes", "git-branch"),
                    RightTab::Graph => ("right-panel-tab-History", "clock"),
                    RightTab::Terminal => ("right-panel-tab-Terminal", "terminal"),
                    RightTab::Codespace => ("right-panel-tab-Codespace", "cloud-upload"),
                };
                let unfolded = match progress {
                    Some((_, p)) if active => p,
                    Some((closing, p)) if tab == closing => 1. - p,
                    _ if active => 1.,
                    _ => 0.,
                };
                let shift = dragging.filter(|(t, _)| *t == tab).map(|(_, o)| o);
                div()
                    .relative()
                    .flex_shrink_0()
                    .flex()
                    .items_stretch()
                    // Keep the measured slot still while its button follows
                    // the pointer; drag offsets are relative to the slot.
                    .child(self.tab_bounds[ix].probe())
                    .child(
                        tab_button(
                            id,
                            TabLabel::from(if tab == RightTab::Codespace {
                                self.codespace_title
                            } else {
                                tab.label()
                            })
                            .max_width(tab_label_width(self.tab_width, count)),
                            path,
                            active,
                            false,
                            unfolded,
                            window,
                        )
                        .relative()
                        .when_some(shift, |d, o| d.left(o))
                        .when(!active && shift.is_some(), |d| {
                            d.opacity(1.).text_color(content(1.))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                this.start_drag(tab, event)
                            }),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if std::mem::take(&mut this.skip_click) {
                                    return;
                                }
                                this.select(tab, cx);
                                this.focus_pane(window, cx);
                            },
                        )),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .id("right-panel-tabs")
            .debug_selector(|| "right-panel-tabs".into())
            .role(Role::TabList)
            .aria_label("Project")
            .h(px(TITLE_BAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(2.))
            .on_mouse_move(
                cx.listener(|this, event: &MouseMoveEvent, _, cx| this.move_drag(event, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_drag(cx)),
            )
            .children(tabs)
            .into_any_element()
    }
}

impl RightPanel {
    /// The banner with the Cloud | Local switch, and under it the connection strip.
    fn cloud_header(&self, chrome: &Chrome, cx: &mut Context<Self>) -> AnyElement {
        let choice = |id: &'static str, label: &'static str, mode: PanelMode| {
            let on = chrome.mode == mode;
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(format!("Show {label} files"))
                .px_2()
                .rounded_md()
                .text_size(px(11.))
                .line_height(px(18.))
                .cursor_pointer()
                .when(on, |d| d.bg(selection()).text_color(content(1.)))
                .when(!on, |d| {
                    d.text_color(content(0.6))
                        .hover(|s| s.bg(content(0.1)))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(PanelEvent::Mode(mode))))
                })
                .child(label)
        };
        let strip = chrome.link.map(|link| {
            let dot = match link {
                Link::Connected => 0x6ee7b7,
                Link::NotConnected => RED_400,
                Link::Starting => AMBER_400,
            };
            div()
                .id("cloud-connection-strip")
                .role(Role::Status)
                .aria_label(link.label())
                .flex()
                .items_center()
                .gap_2()
                .child(div().size(px(6.)).rounded_full().bg(gpui::rgb(dot)))
                .child(link.label())
                .children(chrome.cached.clone().map(|age| {
                    div()
                        .id("cloud-cached-age")
                        .min_w_0()
                        .truncate()
                        .text_color(content(0.45))
                        .child(age)
                }))
                .when(link == Link::NotConnected && chrome.can_connect, |d| {
                    d.child(
                        div()
                            .id("cloud-connect")
                            .role(Role::Button)
                            .aria_label("Connect")
                            .px_2()
                            .rounded_md()
                            .bg(content(0.1))
                            .text_color(content(1.))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.16)))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PanelEvent::Connect)))
                            .child("Connect"),
                    )
                })
        });
        div()
            .id("cloud-files-banner")
            .debug_selector(|| "cloud-files-banner".into())
            .role(Role::Status)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px_3()
            .py(px(6.))
            .border_b_1()
            .border_color(stroke())
            .text_size(px(11.))
            .line_height(px(15.))
            .text_color(content(0.6))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().min_w_0().child(chrome.banner.clone()))
                    .child(
                        div()
                            .id("cloud-panel-mode")
                            .role(Role::Group)
                            .aria_label("Panel mode")
                            .flex()
                            .flex_shrink_0()
                            .gap(px(2.))
                            .child(choice("panel-mode-cloud", "Cloud", PanelMode::Cloud))
                            .child(choice("panel-mode-local", "Local", PanelMode::Local)),
                    ),
            )
            .children(strip)
            .children(
                chrome
                    .setup
                    .as_ref()
                    .map(|setup| self.setup_block(setup, cx)),
            )
            .into_any_element()
    }

    /// "Claude setup": the summary line with the Update action, whether the local setup
    /// changed, and one line per MCP server.
    fn setup_block(&self, setup: &SetupStrip, cx: &mut Context<Self>) -> AnyElement {
        let tone = |tone: SetupTone| match tone {
            SetupTone::Good => content(0.75),
            SetupTone::Notice => gpui::rgb(AMBER_400).into(),
            SetupTone::Bad => gpui::rgb(RED_400).into(),
            SetupTone::Quiet => content(0.45),
        };
        let open = self.setup_open;
        let head = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("cloud-setup-toggle")
                    .debug_selector(|| "cloud-setup-toggle".into())
                    .role(Role::Button)
                    .aria_label("Claude setup")
                    .aria_expanded(open)
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.setup_open = !this.setup_open;
                        cx.notify();
                    }))
                    .child(icon(
                        if open {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        },
                        11.,
                    ))
                    .child(div().text_color(content(0.8)).child("Claude setup"))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(content(0.45))
                            .child(setup.summary.clone()),
                    ),
            )
            .when(setup.can_update, |d| {
                d.child(
                    div()
                        .id("cloud-setup-update")
                        .debug_selector(|| "cloud-setup-update".into())
                        .role(Role::Button)
                        .aria_label("Update Claude setup")
                        .px_2()
                        .rounded_md()
                        .bg(content(0.1))
                        .text_color(content(if setup.updating { 0.5 } else { 1. }))
                        .when(!setup.updating, |d| {
                            d.cursor_pointer().hover(|s| s.bg(content(0.16))).on_click(
                                cx.listener(|_, _, _, cx| cx.emit(PanelEvent::UpdateSetup)),
                            )
                        })
                        .child(if setup.updating {
                            "Updating..."
                        } else {
                            "Update Claude setup"
                        }),
                )
            });
        div()
            .id("cloud-setup")
            .debug_selector(|| "cloud-setup".into())
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(head)
            .when(setup.changed, |d| {
                d.child(
                    div()
                        .id("cloud-setup-changed")
                        .debug_selector(|| "cloud-setup-changed".into())
                        .text_color(gpui::rgb(AMBER_400))
                        .child(SETUP_CHANGED),
                )
            })
            .when_some(setup.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("cloud-setup-error")
                        .role(Role::Alert)
                        .text_color(gpui::rgb(RED_400))
                        .child(error),
                )
            })
            .when(open, |d| {
                d.children(
                    setup
                        .servers
                        .iter()
                        .map(|(name, label, kind)| {
                            let id: SharedString = format!("cloud-setup-server-{name}").into();
                            div()
                                .id(id.clone())
                                .debug_selector(move || id.to_string())
                                .pl_4()
                                .flex()
                                .gap_2()
                                .child(div().text_color(content(0.7)).child(name.clone()))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .truncate()
                                        .text_color(tone(*kind))
                                        .child(label.clone()),
                                )
                        })
                        .collect::<Vec<_>>(),
                )
                .children(
                    setup
                        .warnings
                        .iter()
                        .map(|warning| div().pl_4().text_color(content(0.5)).child(warning.clone()))
                        .collect::<Vec<_>>(),
                )
            })
            .into_any_element()
    }
}

/// Shown instead of the tree while the cloud workspace is being created.
fn workspace_placeholder() -> AnyElement {
    div()
        .id("cloud-files-placeholder")
        .debug_selector(|| "cloud-files-placeholder".into())
        .role(Role::Status)
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .text_color(content(0.5))
        .child("Workspace is starting")
        .into_any_element()
}

impl RightPanel {
    /// While the app has no connection to the workspace, the panes show what was seen last, dimmed
    /// so it never reads as live.
    fn remembered(&self, pane: AnyElement) -> AnyElement {
        if !self.cloud.as_ref().is_some_and(|chrome| chrome.offline) {
            return pane;
        }
        div()
            .id("cloud-remembered")
            .flex_1()
            .min_h_0()
            .w_full()
            .opacity(0.55)
            .child(pane)
            .into_any_element()
    }
    /// The Files or Changes tool of the classic sidebar, with the same cloud chrome as the right
    /// panel: banner, Cloud | Local switch, connection strip, and the placeholder while the
    /// workspace is being created (never a writable local tree for a pending root).
    pub(crate) fn classic_tool(&self, tab: RightTab, cx: &mut Context<Self>) -> AnyElement {
        let chrome = self.cloud.clone();
        let placeholder = chrome.as_ref().is_some_and(|chrome| chrome.placeholder);
        let pane: AnyElement = match tab {
            _ if placeholder => workspace_placeholder(),
            RightTab::Changes => self
                .changes
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            _ => self
                .explorer
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
        };
        let pane = self.remembered(pane);
        let Some(chrome) = chrome else {
            return pane;
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.cloud_header(&chrome, cx))
            .child(div().flex_1().min_h_0().child(pane))
            .into_any_element()
    }
}

impl Render for RightPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let placeholder = self.cloud.as_ref().is_some_and(|chrome| chrome.placeholder)
            && matches!(
                self.tab,
                RightTab::Files | RightTab::Changes | RightTab::Graph
            );
        let pane: AnyElement = match self.tab {
            _ if placeholder => workspace_placeholder(),
            RightTab::Files => self
                .explorer
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            RightTab::Changes => self
                .changes
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            RightTab::Graph => self
                .history
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            RightTab::Codespace => self
                .codespace
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            // The shell paints the terminal over this pane.
            RightTab::Terminal => div().into_any_element(),
        };
        let pane = if self.tab == RightTab::Terminal {
            pane
        } else {
            self.remembered(pane)
        };
        div()
            .id("right-panel")
            .track_focus(&self.focus)
            .role(Role::Group)
            .aria_label("Project tools")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .when(self.top_inset > 0., |d| {
                d.child(
                    div()
                        .h(px(self.top_inset))
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(stroke()),
                )
            })
            .when(self.tab != RightTab::Terminal, |d| {
                d.children(
                    self.cloud
                        .clone()
                        .map(|chrome| self.cloud_header(&chrome, cx)),
                )
            })
            .child(pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tab_order_keeps_known_tabs_once() {
        assert_eq!(
            RightTab::normalize(&[RightTab::Graph, RightTab::Graph]),
            [RightTab::Graph, RightTab::Files]
        );
        assert_eq!(
            RightTab::normalize(&[RightTab::Changes, RightTab::Graph]),
            [RightTab::Graph, RightTab::Files]
        );
        assert_eq!(format_integer(1234567), "1,234,567");
        assert_eq!(format_integer(12), "12");
    }

    /// Draws the classic sidebar's tool the way the shell does.
    struct ClassicProbe(Entity<RightPanel>);
    impl Render for ClassicProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let tool = self
                .0
                .update(cx, |panel, cx| panel.classic_tool(RightTab::Files, cx));
            div().size_full().child(tool)
        }
    }

    #[gpui::test]
    fn the_classic_sidebar_shows_the_banner_the_strip_and_the_placeholder(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::panel_root::{ActiveSession, PanelRoot, resolve};
        use monocode_core::cloud_sessions::CloudRef;
        let mut panel = None;
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
                let entity = cx.new(|cx| RightPanel::new(git, RightTab::Files, vec![], window, cx));
                panel = Some(entity.clone());
                cx.new(|_| ClassicProbe(entity))
            })
            .unwrap()
        });
        let panel = panel.unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let root = |cwd: &str, mode| -> PanelRoot {
            resolve(
                Some(ActiveSession {
                    cwd,
                    worktree_cwd: None,
                    cloud: Some((CloudRef::codespace("octo-abc"), "/work/monocode")),
                }),
                mode,
                false,
                "/o",
            )
        };
        let drawn = |visual: &mut gpui::VisualTestContext| {
            visual.refresh().unwrap();
            (
                visual.debug_bounds("cloud-files-banner").is_some(),
                visual.debug_bounds("cloud-files-placeholder").is_some(),
            )
        };
        // No cloud session: the plain tree.
        assert_eq!(drawn(visual), (false, false));
        let set = |chrome, visual: &mut gpui::VisualTestContext| {
            panel.update(visual, |panel, cx| panel.set_cloud_chrome(chrome, cx))
        };
        let cloud = root(
            "remote://env-octo-abc/workspaces/widgets",
            crate::panel_root::PanelMode::Cloud,
        );
        set(Chrome::of(&cloud, Link::Connected, true), visual);
        assert_eq!(drawn(visual), (true, false));
        // Pending: the placeholder, never the tree, and the tree refuses writes.
        let pending = root(
            "remote://pending-octo-abc/workspaces/widgets",
            crate::panel_root::PanelMode::Cloud,
        );
        set(Chrome::of(&pending, Link::Connected, true), visual);
        assert_eq!(drawn(visual), (true, true));
        panel.update(visual, |panel, cx| {
            assert!(panel.explorer.read(cx).is_read_only())
        });
        let local = root(
            "remote://env-octo-abc/x",
            crate::panel_root::PanelMode::Local,
        );
        set(Chrome::of(&local, Link::Connected, true), visual);
        assert_eq!(drawn(visual), (true, false));
        panel.update(visual, |panel, cx| {
            assert!(!panel.explorer.read(cx).is_read_only())
        });
    }

    #[gpui::test]
    fn the_claude_setup_block_lists_health_and_asks_for_an_update(cx: &mut gpui::TestAppContext) {
        use crate::panel_root::{ActiveSession, SetupStrip, SetupTone, resolve};
        use monocode_core::cloud_sessions::CloudRef;
        let mut panel = None;
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
                let entity = cx.new(|cx| RightPanel::new(git, RightTab::Files, vec![], window, cx));
                panel = Some(entity.clone());
                cx.new(|_| ClassicProbe(entity))
            })
            .unwrap()
        });
        let panel = panel.unwrap();
        let asked = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = asked.clone();
        let _keep = cx.update(|cx| {
            cx.subscribe(&panel, move |_, event: &PanelEvent, _| {
                if matches!(event, PanelEvent::UpdateSetup) {
                    count.set(count.get() + 1);
                }
            })
        });
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let root = resolve(
            Some(ActiveSession {
                cwd: "remote://env-octo-abc/workspaces/widgets",
                worktree_cwd: None,
                cloud: Some((CloudRef::codespace("octo-abc"), "/work/monocode")),
            }),
            crate::panel_root::PanelMode::Cloud,
            false,
            "/o",
        );
        let strip = |changed, updating| SetupStrip {
            summary: "1 skill, 2 MCP servers".into(),
            servers: vec![
                ("github".into(), "connected".into(), SetupTone::Good),
                (
                    "linear".into(),
                    "Sign in in the cloud".into(),
                    SetupTone::Notice,
                ),
            ],
            warnings: vec!["Server \"x\" was not sent.".into()],
            changed,
            can_update: true,
            updating,
            error: None,
        };
        let show = |setup: Option<SetupStrip>, visual: &mut gpui::VisualTestContext| {
            let chrome = Chrome::of(&root, Link::Connected, true).map(|c| c.with_setup(setup));
            panel.update(visual, |panel, cx| panel.set_cloud_chrome(chrome, cx));
            visual.refresh().unwrap();
        };
        // Nothing was sent: no block.
        show(None, visual);
        assert!(visual.debug_bounds("cloud-setup").is_none());
        // Collapsed: the line and the Update action, no change notice yet.
        show(Some(strip(false, false)), visual);
        assert!(visual.debug_bounds("cloud-setup").is_some());
        assert!(visual.debug_bounds("cloud-setup-changed").is_none());
        assert!(visual.debug_bounds("cloud-setup-server-github").is_none());
        // The local setup changed: the indicator shows.
        show(Some(strip(true, false)), visual);
        assert!(visual.debug_bounds("cloud-setup-changed").is_some());
        // Open: one line per server.
        let toggle = visual.debug_bounds("cloud-setup-toggle").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("cloud-setup-server-github").is_some());
        assert!(visual.debug_bounds("cloud-setup-server-linear").is_some());
        // Update is a click; while it runs the button does nothing.
        let update = visual.debug_bounds("cloud-setup-update").unwrap();
        visual.simulate_click(update.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(asked.get(), 1);
        show(Some(strip(true, true)), visual);
        let update = visual.debug_bounds("cloud-setup-update").unwrap();
        visual.simulate_click(update.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(asked.get(), 1);
        // The Local switch has no block.
        let local = resolve(
            Some(ActiveSession {
                cwd: "remote://env-octo-abc/workspaces/widgets",
                worktree_cwd: None,
                cloud: Some((CloudRef::codespace("octo-abc"), "/work/monocode")),
            }),
            crate::panel_root::PanelMode::Local,
            false,
            "/o",
        );
        let chrome = Chrome::of(&local, Link::Connected, true)
            .map(|c| c.with_setup(Some(strip(false, false))));
        assert_eq!(chrome.unwrap().setup, None);
    }

    #[gpui::test]
    fn the_git_status_is_watched_only_while_files_or_changes_is_on_screen(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut parts = None;
        cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
                let panel =
                    cx.new(|cx| RightPanel::new(git.clone(), RightTab::Files, vec![], window, cx));
                parts = Some((git, panel.clone()));
                cx.new(|_| ClassicProbe(panel))
            })
            .unwrap()
        });
        let (git, panel) = parts.unwrap();
        let visible =
            |cx: &mut gpui::TestAppContext| git.update(cx, |g, _| g.panel_visible_for_test());
        // Closed: nothing is on screen.
        assert!(!visible(cx));
        // The right panel (modern layout).
        panel.update(cx, |p, cx| p.set_shown(true, cx));
        assert!(visible(cx));
        panel.update(cx, |p, cx| p.select(RightTab::Changes, cx));
        assert!(visible(cx));
        // Open on the terminal or the codespace tab: a long-poll would serve nothing.
        panel.update(cx, |p, cx| p.select(RightTab::Terminal, cx));
        assert!(!visible(cx));
        panel.update(cx, |p, cx| p.select(RightTab::Codespace, cx));
        assert!(!visible(cx));
        panel.update(cx, |p, cx| p.select(RightTab::Graph, cx));
        assert!(!visible(cx));
        // The classic layout: the panel is closed, the left sidebar shows Files or Changes.
        panel.update(cx, |p, cx| {
            p.set_shown(false, cx);
            p.select(RightTab::Files, cx);
        });
        assert!(!visible(cx));
        panel.update(cx, |p, cx| p.set_sidebar_tab(Some(RightTab::Files), cx));
        assert!(visible(cx));
        panel.update(cx, |p, cx| p.set_sidebar_tab(Some(RightTab::Changes), cx));
        assert!(visible(cx));
        panel.update(cx, |p, cx| p.set_sidebar_tab(None, cx));
        assert!(!visible(cx));
    }

    #[test]
    fn only_files_and_changes_count_as_the_status_being_on_screen() {
        assert!(status_visible(true, RightTab::Files));
        assert!(status_visible(true, RightTab::Changes));
        for tab in [RightTab::Graph, RightTab::Terminal, RightTab::Codespace] {
            assert!(!status_visible(true, tab));
        }
        assert!(!status_visible(false, RightTab::Files));
    }
}
