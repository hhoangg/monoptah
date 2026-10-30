//! The pieces the canvas positions: node cards, For each frames, the strips above the lanes of a
//! run and the loop edge's label. Sizes follow the zoom (there is no transform in GPUI), so
//! every length goes through `scaled`.
use super::super::review::node_title;
use super::canvas::{GraphCanvas, GraphEvent};
use super::hit::{HandleKind, handles_of};
use super::scene::{Bar, BarKind, EdgeLabel, LABEL_H, SceneNode, TabItem, out_at};
use crate::monos::*;
use gpui::{BoxShadow, ClickEvent, Hsla, MouseDownEvent, Role, ScrollHandle, ScrollWheelEvent};
use monocode_core::mono::workflows::{
    board_model::{FlowKind, GROUP_HEADER, NODE_H, Point, out_handles},
    model::{Decide, Node, NodeKind},
    run_graph_model::RunNodeStatus,
};

const EMERALD: u32 = 0x34d399;
const AMBER: u32 = 0xfbbf24;
const RED: u32 = 0xef4444;
/// About how wide a character of a port label is, at its 11 pixel size.
const PORT_CHAR: f32 = 6.;
/// How far right of its out a port label starts.
const PORT_LABEL_GAP: f32 = 9.;

fn hue(color: u32, alpha: f32) -> Hsla {
    Hsla::from(rgb(color)).opacity(alpha)
}

/// The icon of a node type.
pub(in crate::monos) fn type_icon(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Schedule { .. } => "clock",
        NodeKind::Manual => "play",
        NodeKind::Session { .. } => "message-multiple",
        NodeKind::Command { .. } => "terminal",
        NodeKind::AskMono { .. } => "circle-help",
        NodeKind::If { .. } | NodeKind::Switch { .. } => "git-branch",
        NodeKind::ForEach { .. } => "refresh-cw",
        NodeKind::Report { .. } => "paper-plane",
    }
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

/// The second line of a node's card: the part of its setup that says what it does.
pub(in crate::monos) fn node_summary(node: &Node) -> String {
    let or = |text: &str, none: &str| {
        let line = first_line(text);
        if line.is_empty() { none } else { line }.to_string()
    };
    match &node.kind {
        NodeKind::Schedule { schedule } => schedule.label(),
        NodeKind::Manual => "Starts when you run it".into(),
        NodeKind::Session { prompt, .. } => or(prompt, "No prompt yet"),
        NodeKind::Command { command, .. } => or(command, "No command yet"),
        NodeKind::AskMono { question, .. } => or(question, "No question yet"),
        NodeKind::If {
            decide, condition, ..
        } => {
            if *decide == Some(Decide::Mono) {
                "Mono decides".into()
            } else {
                or(condition.as_deref().unwrap_or(""), "No condition yet")
            }
        }
        NodeKind::Switch {
            decide,
            value,
            cases,
            ..
        } => {
            if *decide == Some(Decide::Mono) {
                "Mono decides".into()
            } else if cases.is_empty() {
                "No cases yet".into()
            } else {
                or(value.as_deref().unwrap_or(""), "No value yet")
            }
        }
        NodeKind::ForEach { list, .. } => {
            if list.is_empty() {
                "No list yet".into()
            } else {
                list.clone()
            }
        }
        NodeKind::Report { template } => if template.as_deref().is_some_and(|t| !t.is_empty()) {
            "Custom report"
        } else {
            "Summary of every branch"
        }
        .into(),
    }
}

/// "Done · 12s · round 2" without its round: the badge beside the title says that.
pub(in crate::monos) fn without_round(label: &str) -> &str {
    match label.rfind(" \u{b7} round ") {
        Some(at)
            if !label[at + " \u{b7} round ".len()..].is_empty()
                && label[at + " \u{b7} round ".len()..]
                    .bytes()
                    .all(|b| b.is_ascii_digit()) =>
        {
            &label[..at]
        }
        _ => label,
    }
}

/// How wide the label right of out number `index` of `count` on a card `card_w` wide may be: the
/// room up to the next out, or up to the card's right edge for the last one.
pub(in crate::monos) fn port_label_width(card_w: f32, index: usize, count: usize) -> f32 {
    let from = out_at(index, count);
    let to = if index + 1 < count {
        out_at(index + 1, count)
    } else {
        1.
    };
    (card_w * (to - from) - PORT_LABEL_GAP).max(PORT_CHAR * 2.)
}

/// `name` cut with an ellipsis to fit `width` (graph pixels) at the size of the port labels.
pub(in crate::monos) fn port_label(name: &str, width: f32) -> String {
    let fit = ((width / PORT_CHAR) as usize).max(2);
    if name.chars().count() <= fit {
        return name.to_string();
    }
    let cut: String = name.chars().take(fit - 1).collect();
    format!("{cut}\u{2026}")
}

fn handle_name(kind: &HandleKind) -> &str {
    match kind {
        HandleKind::In => "in",
        HandleKind::Out(None) => "out",
        HandleKind::Out(Some(name)) => name,
        HandleKind::LoopIn => "loop-in",
        HandleKind::LoopOut => "loop-out",
    }
}

/// How a card looks for the state of its node in the run.
struct Look {
    border: Hsla,
    dashed: bool,
    ring: Option<(f32, Hsla)>,
    faded: bool,
}

impl GraphCanvas {
    fn look(&self, status: Option<RunNodeStatus>, selected: bool) -> Look {
        let accent = self.color;
        let mut look = Look {
            border: content(0.1),
            dashed: false,
            ring: None,
            faded: false,
        };
        match status {
            Some(RunNodeStatus::Done) => look.border = hue(EMERALD, 0.45),
            Some(RunNodeStatus::Running) => {
                look.border = hue(accent, 0.7);
                look.ring = Some((3., hue(accent, 0.14)));
            }
            Some(RunNodeStatus::Waiting) => look.border = hue(AMBER, 0.55),
            Some(RunNodeStatus::Failed) => look.border = hue(RED, 0.55),
            Some(RunNodeStatus::Skipped | RunNodeStatus::Pending) => {
                look.dashed = true;
                look.faded = true;
            }
            Some(RunNodeStatus::Stopped) | None => {}
        }
        // The picked card keeps the accent ring over its status border.
        if selected {
            look.border = hue(accent, 0.65);
            look.ring = Some((1., hue(accent, 0.3)));
        }
        look
    }

    /// The dot that says how a node stands in a run.
    fn status_dot(&self, status: RunNodeStatus, side: f32, id: &str) -> AnyElement {
        let side = self.scaled(side);
        let disc = |color: Hsla| {
            div()
                .size(px(side))
                .flex_shrink_0()
                .rounded_full()
                .bg(color)
                .into_any_element()
        };
        match status {
            RunNodeStatus::Running => super::super::view::running_dot(
                SharedString::from(format!("wf-pulse-{id}")),
                self.color,
                side,
            ),
            RunNodeStatus::Done => disc(hue(EMERALD, 0.8)),
            RunNodeStatus::Waiting => disc(hue(AMBER, 0.8)),
            RunNodeStatus::Failed => disc(hue(RED, 0.7)),
            RunNodeStatus::Stopped => disc(content(0.4)),
            RunNodeStatus::Skipped | RunNodeStatus::Pending => div()
                .size(px(side))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(content(0.3))
                .into_any_element(),
        }
    }

    fn picked(&self, node: &SceneNode) -> bool {
        node.flow.selected || (node.key.is_some() && node.key == self.selected)
    }

    /// Pressing a card on the board starts dragging it; letting go without moving is a click.
    fn press(
        &mut self,
        node: &SceneNode,
    ) -> impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static {
        let (id, origin) = (
            node.flow.id.clone(),
            Point {
                x: node.rect.x,
                y: node.rect.y,
            },
        );
        move |this, event, window, cx| {
            let focus = this.root_focus(cx);
            window.focus(&focus, cx);
            this.begin_drag(&id, origin, event.position);
            cx.stop_propagation();
        }
    }

    /// The small rings on the sides of a card on the board: pressing one starts a connection.
    /// Lines come out of the bottom (an If has two) and loops leave and arrive at the right;
    /// while a connection is drawn the ones it could land on light up.
    fn handle_dots(&mut self, node: &SceneNode, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !self.editable {
            return Vec::new();
        }
        let connecting = self.connection.clone();
        let hit = self.scaled(16.).max(10.);
        let group: SharedString = format!("wf-card-{}", node.flow.id).into();
        let data = &node.flow.data;
        handles_of(node)
            .into_iter()
            .map(|handle| {
                let (x, y) = (handle.at.x - node.rect.x, handle.at.y - node.rect.y);
                let leaves = matches!(handle.kind, HandleKind::Out(_) | HandleKind::LoopOut);
                let used = match handle.kind {
                    HandleKind::LoopOut => data.loop_out,
                    HandleKind::LoopIn => data.loop_in,
                    _ => false,
                };
                let target = connecting.as_ref().is_some_and(|c| {
                    let from = match &c.kind {
                        HandleKind::Out(h) => h.as_deref(),
                        _ => None,
                    };
                    !leaves && self.may_connect(&c.source, from, &node.flow.id)
                });
                let rest = match handle.kind {
                    HandleKind::Out(_) => 0.5,
                    HandleKind::LoopOut | HandleKind::LoopIn if used => 1.,
                    _ => 0.,
                };
                let ring = div()
                    .size(self.px(8.))
                    .rounded_full()
                    .border_1()
                    .border_color(if target {
                        hue(self.color, 1.)
                    } else if matches!(handle.kind, HandleKind::LoopOut | HandleKind::LoopIn) {
                        hue(self.color, 0.8)
                    } else {
                        content(0.38)
                    })
                    .bg(pane_fill());
                let id = node.flow.id.clone();
                let kind = handle.kind.clone();
                let selector = format!("wf-handle-{}-{}", node.flow.id, handle_name(&handle.kind));
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .absolute()
                    .left(self.px(x) - px(hit / 2.))
                    .top(self.px(y) - px(hit / 2.))
                    .size(px(hit))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(if target || connecting.is_some() && !leaves {
                        1.
                    } else {
                        rest
                    })
                    .when(rest < 1. && !target && connecting.is_none(), |d| {
                        d.group_hover(group.clone(), |s| s.opacity(1.))
                    })
                    .when(leaves, |d| {
                        d.cursor_crosshair().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                let focus = this.root_focus(cx);
                                window.focus(&focus, cx);
                                this.begin_connection(&id, kind.clone(), handle.at, event.position);
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        )
                    })
                    .child(ring)
                    .into_any_element()
            })
            .collect()
    }

    fn click(&mut self, key: &Option<String>, event: &ClickEvent, cx: &mut Context<Self>) {
        if self.editable || !Self::is_click(event) {
            return;
        }
        if let Some(key) = key {
            cx.emit(GraphEvent::Select(key.clone()));
        }
    }

    pub(super) fn node(&mut self, node: &SceneNode, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match node.flow.kind {
            FlowKind::Group => vec![self.frame(node, cx)],
            FlowKind::Board => self.card(node, cx),
        }
    }

    /// A trigger, action, If or Report node: an icon tile and two lines.
    fn card(&mut self, node: &SceneNode, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let data = &node.flow.data;
        let run = data.run.clone();
        let compact = run.is_some() && data.compact;
        let status = run.as_ref().map(|r| r.status);
        let selected = self.picked(node);
        let mut look = self.look(status, selected);
        let id = node.flow.id.clone();
        // The card a connection being drawn would land on.
        if self
            .connection
            .as_ref()
            .is_some_and(|c| c.hover.as_deref() == Some(id.as_str()))
        {
            look.border = hue(self.color, 0.9);
            look.ring = Some((2., hue(self.color, 0.25)));
        }
        let title = node_title(&data.node);
        let (left, top) = self.at(Point {
            x: node.rect.x,
            y: node.rect.y,
        });
        let issues = &data.issues;
        let second = match &run {
            Some(run) => (without_round(&run.label).to_string(), content(0.4)),
            None if !issues.is_empty() => (
                format!(
                    "{}{}",
                    issues[0],
                    if issues.len() > 1 {
                        format!(" (+{})", issues.len() - 1)
                    } else {
                        String::new()
                    }
                ),
                hue(RED, 0.8),
            ),
            None => (node_summary(&data.node), content(0.4)),
        };
        let badge = run.as_ref().filter(|r| r.round > 1).map(|r| {
            if compact {
                format!("r{}", r.round)
            } else {
                format!("round {}", r.round)
            }
        });
        let accent = hue(self.color, 1.);
        let tile = div()
            .size(self.px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(self.px(6.))
            .bg(content(0.06))
            .child(icon(type_icon(&data.node.kind), self.scaled(14.)).text_color(content(0.6)));
        let heading = div()
            .flex()
            .items_center()
            .gap(self.px(6.))
            .min_w_0()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(self.px(12.))
                    .line_height(self.px(20.))
                    .text_color(content(0.85))
                    .child(title.clone()),
            )
            .when_some(badge, |d, badge| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .rounded_full()
                        .px(self.px(6.))
                        .bg(hue(self.color, 0.14))
                        .text_size(self.px(10.))
                        .line_height(self.px(16.))
                        .text_color(accent)
                        .child(badge),
                )
            });
        let lines = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(heading)
            .child(
                div()
                    .truncate()
                    .text_size(self.px(11.))
                    .line_height(self.px(16.))
                    .text_color(second.1)
                    .child(second.0),
            );
        let trailing = match status {
            Some(status) if !compact => Some(self.status_dot(status, 8., &id)),
            None if !issues.is_empty() => Some(
                div()
                    .size(self.px(8.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(hue(RED, 0.7))
                    .into_any_element(),
            ),
            _ => None,
        };
        let inner = div()
            .size_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(self.px(10.))
            .when(look.faded, |d| d.opacity(0.55))
            .when(!compact, |d| d.child(tile))
            .child(lines)
            .children(trailing);
        let mut shadows = Vec::new();
        if let Some((spread, color)) = look.ring {
            shadows.push(BoxShadow::new(px(0.), px(0.), color).spread_radius(px(spread)));
        }
        if node.stacked {
            let base = pane_fill();
            for (step, alpha) in [(10., 0.06), (5., 0.1)] {
                shadows.push(BoxShadow::new(px(step), px(step), content(alpha)));
                shadows.push(BoxShadow::new(px(step), px(step), base).spread_radius(px(-1.)));
            }
        }
        let key = node.key.clone();
        let selector = format!("wf-node-{id}");
        let focus = self
            .focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let pick_key = node.key.clone();
        let editable = self.editable;
        let pressed = editable.then(|| self.press(node));
        let handles = self.handle_dots(node, cx);
        let group: SharedString = format!("wf-card-{id}").into();
        let card_id = id.clone();
        let card = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .group(group)
            .role(Role::Button)
            .aria_label(title)
            .track_focus(&focus)
            .tab_index(0)
            .on_key_down(cx.listener(move |_, event: &gpui::KeyDownEvent, _, cx| {
                // Enter and Space alone: a shortcut with them (Cmd+Enter) is someone else's.
                if event.keystroke.modifiers.modified() {
                    return;
                }
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    if editable {
                        cx.emit(GraphEvent::Click(card_id.clone()));
                    } else if let Some(key) = &pick_key {
                        cx.emit(GraphEvent::Select(key.clone()));
                    }
                    cx.stop_propagation();
                }
            }))
            .absolute()
            .left(left)
            .top(top)
            .w(self.px(node.rect.w))
            .h(self.px(node.rect.h))
            .px(self.px(8.))
            .py(self.px(if compact { 4. } else { 5. }))
            .rounded(self.px(8.))
            .border_1()
            .border_color(look.border)
            .when(look.dashed, |d| d.border_dashed())
            .when(!look.dashed, |d| d.bg(content(0.05)))
            .when(!shadows.is_empty(), |d| d.shadow(shadows))
            .cursor_pointer()
            .when_some(pressed, |d, press| {
                d.on_mouse_down(MouseButton::Left, cx.listener(press))
            })
            .on_click(
                cx.listener(move |this, event: &ClickEvent, _, cx| this.click(&key, event, cx)),
            )
            .child(inner)
            .children(handles)
            .into_any_element();
        let mut out = vec![card];
        // Under an If, which line is the true one and which the false one; under a Switch, which
        // line is which case. Each label starts just right of its out.
        let outs = out_handles(&data.node);
        for (index, word) in outs.iter().enumerate() {
            let at = out_at(index, outs.len());
            let (x, y) = self.at(Point {
                x: node.rect.x + node.rect.w * at + PORT_LABEL_GAP,
                y: node.rect.y + NODE_H + 5.,
            });
            let width = port_label_width(node.rect.w, index, outs.len());
            out.push(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(self.px(width))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(self.px(11.))
                    .line_height(self.px(14.))
                    .text_color(content(0.42))
                    .child(port_label(word, width))
                    .into_any_element(),
            );
        }
        out
    }

    /// A For each: a frame around the nodes that run once per item.
    fn frame(&mut self, node: &SceneNode, cx: &mut Context<Self>) -> AnyElement {
        let data = &node.flow.data;
        let run = data.run.clone();
        let selected = self.picked(node);
        let (left, top) = self.at(Point {
            x: node.rect.x,
            y: node.rect.y,
        });
        let id = node.flow.id.clone();
        let mut border = content(0.1);
        match run.as_ref().map(|r| r.status) {
            Some(RunNodeStatus::Running) => border = hue(self.color, 0.35),
            Some(RunNodeStatus::Failed) => border = hue(RED, 0.35),
            Some(RunNodeStatus::Done) => border = hue(EMERALD, 0.28),
            _ => {}
        }
        if selected {
            border = hue(self.color, 0.65);
        }
        let list = match &data.node.kind {
            NodeKind::ForEach { list, .. } => list.clone(),
            _ => String::new(),
        };
        let named = data
            .node
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map_or(list, str::to_string);
        let caption = match (&run, data.run_items) {
            (Some(_), Some(count)) => format!("{named} \u{b7} {count}"),
            _ => named,
        };
        let header = div()
            .h(self.px(GROUP_HEADER))
            .px(self.px(12.))
            .flex()
            .items_center()
            .gap(self.px(6.))
            .text_size(self.px(11.))
            .child(icon("refresh-cw", self.scaled(12.)).text_color(content(0.4)))
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(content(0.4))
                    .child("FOR EACH"),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(content(0.55))
                    .child(caption),
            )
            .when_some(run.as_ref(), |d, run| {
                d.child(
                    div()
                        .ml_auto()
                        .min_w_0()
                        .truncate()
                        .text_color(content(0.4))
                        .child(run.label.clone()),
                )
                .child(self.status_dot(run.status, 8., &id))
            })
            .when(run.is_none() && !data.issues.is_empty(), |d| {
                d.child(
                    div()
                        .ml_auto()
                        .size(self.px(8.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(hue(RED, 0.7)),
                )
            });
        let key = node.key.clone();
        let selector = format!("wf-node-{id}");
        let pressed = self.editable.then(|| self.press(node));
        let handles = self.handle_dots(node, cx);
        let group: SharedString = format!("wf-card-{id}").into();
        let hovered = self
            .connection
            .as_ref()
            .is_some_and(|c| c.hover.as_deref() == Some(id.as_str()));
        if hovered {
            border = hue(self.color, 0.9);
        }
        let empty_hint = (self.editable && data.empty).then(|| {
            div()
                .px(self.px(12.))
                .pt(self.px(4.))
                .text_size(self.px(11.))
                .line_height(self.px(16.))
                .text_color(content(0.35))
                .child("Drag nodes in to run them once per item.")
        });
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .group(group)
            .role(Role::Button)
            .aria_label(node_title(&data.node))
            .absolute()
            .left(left)
            .top(top)
            .w(self.px(node.rect.w))
            .h(self.px(node.rect.h))
            .rounded(self.px(12.))
            .border_1()
            .border_color(border)
            .bg(content(0.03))
            .when(selected, |d| {
                d.shadow(vec![
                    BoxShadow::new(px(0.), px(0.), hue(self.color, 0.3)).spread_radius(px(1.)),
                ])
            })
            .cursor_pointer()
            .when_some(pressed, |d, press| {
                d.on_mouse_down(MouseButton::Left, cx.listener(press))
            })
            .on_click(
                cx.listener(move |this, event: &ClickEvent, _, cx| this.click(&key, event, cx)),
            )
            .child(header)
            .children(empty_hint)
            .children(handles)
            .into_any_element()
    }

    /// The pill on the loop edge: who decides and how many rounds at most.
    pub(super) fn edge_label(&self, id: &str, label: &EdgeLabel) -> AnyElement {
        let (left, top) = self.at(Point {
            x: label.at.x,
            y: label.at.y - LABEL_H / 2.,
        });
        let full: SharedString = label.full.clone().into();
        let selector = format!("wf-loop-label-{id}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .absolute()
            .left(left)
            .top(top)
            .h(self.px(LABEL_H))
            .flex()
            .items_center()
            .px(self.px(6.))
            .rounded_full()
            .bg(hue(self.color, 0.12))
            .text_size(self.px(11.))
            .line_height(self.px(16.))
            .text_color(hue(self.color, 1.))
            .whitespace_nowrap()
            .tooltip(move |_, cx| cx.new(|_| Tip(full.clone())).into())
            .child(label.text.clone())
            .into_any_element()
    }

    /// A strip above the lanes: a lane's title, the item tabs, or the round selector.
    pub(super) fn bar(&mut self, bar: &Bar, cx: &mut Context<Self>) -> AnyElement {
        let (left, top) = self.at(Point {
            x: bar.rect.x,
            y: bar.rect.y,
        });
        let frame = div()
            .absolute()
            .left(left)
            .top(top)
            .w(self.px(bar.rect.w))
            .h(self.px(bar.rect.h))
            .flex()
            .items_center();
        match &bar.kind {
            BarKind::Lane { title, status } => frame
                .gap(self.px(6.))
                .px(self.px(4.))
                .text_size(self.px(11.))
                .line_height(self.px(18.))
                .text_color(content(0.55))
                .child(self.tab_dot(*status))
                .child(div().min_w_0().truncate().child(title.clone()))
                .into_any_element(),
            BarKind::Tabs {
                group,
                items,
                selected,
            } => {
                let tabs: Vec<_> = items
                    .iter()
                    .map(|item| {
                        let (group, index) = (group.clone(), item.index);
                        (
                            format!("wf-tab-{group}-{index}"),
                            item.index == *selected,
                            item,
                            Box::new(move |cx: &mut Context<Self>| {
                                cx.emit(GraphEvent::PickItem {
                                    group: group.clone(),
                                    index,
                                })
                            }) as Box<dyn Fn(&mut Context<Self>)>,
                        )
                    })
                    .collect();
                let at = items.iter().position(|i| i.index == *selected);
                self.strip(frame, &bar.id, "wf-tabs", at, tabs, cx)
            }
            BarKind::Rounds {
                group,
                rounds,
                selected,
            } => {
                let items: Vec<TabItem> = rounds
                    .iter()
                    .map(|r| TabItem {
                        index: *r as usize,
                        title: format!("Round {r}"),
                        status: None,
                    })
                    .collect();
                let tabs: Vec<_> = items
                    .iter()
                    .map(|item| {
                        let (group, round) = (group.clone(), item.index as u32);
                        (
                            format!("wf-round-{group}-{round}"),
                            round == *selected,
                            item,
                            Box::new(move |cx: &mut Context<Self>| {
                                cx.emit(GraphEvent::PickRound {
                                    group: group.clone(),
                                    round,
                                })
                            }) as Box<dyn Fn(&mut Context<Self>)>,
                        )
                    })
                    .collect();
                let at = rounds.iter().position(|r| r == selected);
                // The tabs sit at the right end of the strip.
                self.strip(frame.justify_end(), &bar.id, "wf-rounds", at, tabs, cx)
            }
        }
    }

    fn tab_dot(&self, status: RunNodeStatus) -> AnyElement {
        let dot = self.scaled(6.);
        let disc = |color: Hsla| {
            div()
                .size(px(dot))
                .flex_shrink_0()
                .rounded_full()
                .bg(color)
                .into_any_element()
        };
        match status {
            RunNodeStatus::Running => disc(hue(self.color, 1.)),
            RunNodeStatus::Done => disc(hue(EMERALD, 0.8)),
            RunNodeStatus::Waiting => disc(hue(AMBER, 0.8)),
            RunNodeStatus::Failed => disc(hue(RED, 0.7)),
            RunNodeStatus::Stopped => disc(content(0.4)),
            RunNodeStatus::Skipped | RunNodeStatus::Pending => div()
                .size(px(dot))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(content(0.3))
                .into_any_element(),
        }
    }

    /// A row of tabs that scrolls sideways on its own, keeping the picked tab in view.
    #[allow(clippy::type_complexity)]
    fn strip(
        &mut self,
        frame: gpui::Div,
        id: &str,
        label: &'static str,
        picked: Option<usize>,
        tabs: Vec<(String, bool, &TabItem, Box<dyn Fn(&mut Context<Self>)>)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strip = self.strips.entry(id.to_string()).or_default();
        if strip.shown != picked {
            if let Some(at) = picked {
                strip.handle.scroll_to_item(at);
            }
            strip.shown = picked;
        }
        let handle: ScrollHandle = strip.handle.clone();
        let tab_elements: Vec<AnyElement> = tabs
            .into_iter()
            .map(|(selector, on, item, pick)| {
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .role(Role::Tab)
                    .aria_selected(on)
                    .aria_label(item.title.clone())
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(self.px(6.))
                    .max_w(self.px(160.))
                    .px(self.px(8.))
                    .py(self.px(2.))
                    .rounded(self.px(6.))
                    .text_size(self.px(11.))
                    .line_height(self.px(18.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .text_color(if on { content(1.) } else { content(0.6) })
                    .bg(if on {
                        hue(self.color, 0.16)
                    } else {
                        content(0.05)
                    })
                    .when(!on, |d| d.hover(|s| s.bg(content(0.09))))
                    .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                        if Self::is_click(event) {
                            pick(cx);
                        }
                    }))
                    .when_some(item.status, |d, status| d.child(self.tab_dot(status)))
                    .child(div().min_w_0().truncate().child(item.title.clone()))
                    .into_any_element()
            })
            .collect();
        frame
            .child(
                div()
                    .id(SharedString::from(format!("{label}-{id}")))
                    .debug_selector({
                        let selector = format!("{label}-{id}");
                        move || selector.clone()
                    })
                    .role(Role::TabList)
                    .aria_label(if label == "wf-rounds" {
                        "Rounds"
                    } else {
                        "Items"
                    })
                    .relative()
                    .min_w_0()
                    .max_w_full()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(self.px(4.))
                    .overflow_x_scroll()
                    .track_scroll(&handle)
                    // The strip scrolls sideways by itself; the wheel over it must not also pan the
                    // canvas.
                    .on_scroll_wheel(|_: &ScrollWheelEvent, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                        cx.stop_propagation()
                    })
                    .children(tab_elements),
            )
            .into_any_element()
    }
}

/// A tooltip with a line of text.
struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(320.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(shell_fill())
            .border_1()
            .border_color(content(0.12))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(content(0.85))
            .child(self.0.clone())
    }
}
