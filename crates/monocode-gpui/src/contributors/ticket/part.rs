//! Parts of the Ticket tab that are drawn from a cache while the pointer moves
//! over a chart or a calendar. A chart hover notifies only the chart, but gpui
//! redraws every view that is not cached, so the leaderboard rows, the heatmap
//! and the member's ticket rows (the heavy parts) would be built again for each
//! bucket crossed. Each part is an entity that draws what the tab already knows
//! how to draw (`TicketView::part_body`); the tab notifies it when its own state
//! changes (`push_board`), and a hover frame replays it.
//!
//! The cache needs a height up front and these parts grow (an opened member, a
//! ticket's reasons), so the height is the one measured at the last draw. When
//! it changes, the tab is asked to draw again, so the next frame uses it. A
//! part is drawn afresh until its first measure, and when accessibility is on
//! (replayed frames carry no accessibility nodes).
use super::{overview::heatmap, state::TicketView};
use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, StyleRefinement, WeakEntity, Window, canvas,
    div, prelude::*, px,
};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PartKind {
    /// The member by level heatmap.
    Heat,
    /// The leaderboard.
    Leaderboard,
    /// The tickets of the member whose page is open.
    Tickets,
}

pub(super) struct Part {
    host: WeakEntity<TicketView>,
    kind: PartKind,
    /// The height at the last draw, shared with the probe that measures it.
    height: Rc<Cell<f32>>,
}

impl Part {
    pub(super) fn new(host: WeakEntity<TicketView>, kind: PartKind) -> Self {
        Self {
            host,
            kind,
            height: Rc::default(),
        }
    }

    /// The height to lay the cache out at; 0 before the first draw.
    pub(super) fn height(&self) -> f32 {
        self.height.get()
    }
}

impl Render for Part {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        let body = match self.host.upgrade() {
            Some(host) => host.update(cx, |tab, tab_cx| tab.part_body(kind, tab_cx)),
            None => div().into_any_element(),
        };
        let (height, host) = (self.height.clone(), self.host.clone());
        // Measures the part; a change redraws the tab, which lays the cache out at it.
        let probe = canvas(
            move |bounds, window, cx| {
                let now = bounds.size.height.as_f32();
                if (height.get() - now).abs() > 0.5 {
                    height.set(now);
                    let host = host.clone();
                    window.defer(cx, move |_, cx| {
                        let _ = host.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div().relative().w_full().child(probe).child(body)
    }
}

/// The part as an element: cached once it has a height, else drawn afresh.
pub(super) fn shown(part: &Entity<Part>, a11y: bool, cx: &gpui::App) -> AnyElement {
    let height = part.read(cx).height();
    if a11y || height <= 0. {
        return part.clone().into_any_element();
    }
    part.clone()
        .cached(StyleRefinement::default().w_full().h(px(height)))
        .into_any_element()
}

impl TicketView {
    /// What a part draws.
    pub(super) fn part_body(&mut self, kind: PartKind, cx: &mut Context<Self>) -> AnyElement {
        match kind {
            PartKind::Heat => heatmap(&self.derived.matrix).into_any_element(),
            PartKind::Leaderboard => self.leaderboard(cx),
            PartKind::Tickets => {
                let row = self
                    .detail
                    .as_ref()
                    .and_then(|id| self.derived.board.members.iter().find(|m| &m.id == id))
                    .cloned();
                match row {
                    Some(row) => self.member_tickets(&row, cx),
                    None => div().into_any_element(),
                }
            }
        }
    }

    /// The entity of a part, made on first use.
    pub(super) fn part(&mut self, kind: PartKind, cx: &mut Context<Self>) -> AnyElement {
        let slot = match kind {
            PartKind::Heat => 0,
            PartKind::Leaderboard => 1,
            PartKind::Tickets => 2,
        };
        let entity = match self.overview.parts[slot].clone() {
            Some(entity) => entity,
            None => {
                let host = cx.weak_entity();
                let entity = cx.new(|_| Part::new(host, kind));
                self.overview.parts[slot] = Some(entity.clone());
                entity
            }
        };
        shown(&entity, self.ui.a11y, cx)
    }

    /// Tells the parts the tab changed. Never call this while drawing.
    pub(super) fn push_parts(&mut self, cx: &mut Context<Self>) {
        for part in self.overview.parts.iter().flatten() {
            part.update(cx, |_, cx| cx.notify());
        }
    }
}
