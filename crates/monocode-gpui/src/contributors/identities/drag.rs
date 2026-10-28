//! The drag of an identity onto another person (`useIdentityDrag.ts` and
//! `DragGhost.tsx`). GPUI starts the drag itself once the pointer has moved a
//! couple of pixels from the press (the TS threshold was 4px, so a plain click
//! stays a click), follows the pointer with the view `DraggedIdentity` renders
//! and ends the gesture on release; `DragState` keeps the rules: which person is
//! under the pointer, which drops count, and when the gesture is over.
use crate::theme::*;
use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px};
use monocode_core::contributors::model::unmapped::PersonRow;

/// What travels with the pointer: the identity and the person it comes from.
#[derive(Clone)]
pub(in crate::contributors) struct DraggedIdentity {
    pub key: String,
    pub from: String,
}

/// The label that follows the pointer (`DragGhost`).
impl Render for DraggedIdentity {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "contributors-id-ghost".into())
            .max_w(px(256.))
            .truncate()
            .rounded_md()
            .border_1()
            .border_color(accent_color().opacity(0.6))
            .bg(floating_fill())
            .px_2()
            .py_1()
            .font_family(crate::fonts::mono())
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(content(1.))
            .child(self.key.clone())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Active {
    key: String,
    from: String,
    /// The person whose card is under the pointer, if any.
    over: Option<String>,
}

/// Present only while an identity is being dragged.
#[derive(Default)]
pub(in crate::contributors) struct DragState {
    active: Option<Active>,
}

impl DragState {
    pub fn start(&mut self, key: &str, from: &str) {
        self.active = Some(Active {
            key: key.into(),
            from: from.into(),
            over: None,
        });
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The pointer is over this person's card (or over none).
    pub fn hover(&mut self, person: Option<&str>) {
        if let Some(active) = &mut self.active {
            active.over = person.map(String::from);
        }
    }

    /// The pointer left this card; another card may already have taken over.
    pub fn leave(&mut self, person: &str) {
        if let Some(active) = &mut self.active
            && active.over.as_deref() == Some(person)
        {
            active.over = None;
        }
    }

    pub fn over(&self) -> Option<&str> {
        self.active.as_ref()?.over.as_deref()
    }

    pub fn key(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.key.as_str())
    }

    /// The card shows it takes the drop: it is under the pointer and does not
    /// already hold the identity.
    pub fn highlights(&self, row: &PersonRow) -> bool {
        let Some(active) = &self.active else {
            return false;
        };
        active.over.as_deref() == Some(row.id.as_str())
            && !row.identities.iter().any(|i| i.key == active.key)
    }

    /// The row of this identity is dimmed while it is dragged.
    pub fn dims(&self, key: &str) -> bool {
        self.key() == Some(key)
    }

    /// The release over `person`: the identity and the target when the drop
    /// counts. Dropping on the person it comes from does nothing. The gesture is over.
    pub fn drop_on(&mut self, person: &str) -> Option<(String, String)> {
        let active = self.active.take()?;
        (active.from != person).then(|| (active.key, person.to_string()))
    }

    /// Escape, a release over nothing, or any other end of the gesture.
    pub fn cancel(&mut self) {
        self.active = None;
    }
}
