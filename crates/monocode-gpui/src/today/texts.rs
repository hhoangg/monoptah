//! Read-only text in the detail panel (note bodies, the context summary): one
//! shared [`Input`] each, so the text can be selected, copied and its links
//! clicked.
use super::Today;
use crate::input::Input;
use gpui::{AppContext, Context, Entity};
use std::{cell::RefCell, collections::HashMap};

#[derive(Default)]
pub(crate) struct Texts {
    views: RefCell<HashMap<String, Entity<Input>>>,
}
impl Texts {
    /// The field for `key`, created on first use and refilled only when its
    /// text changed, so an open selection survives a reload.
    pub(super) fn view(
        &self,
        key: &str,
        label: &'static str,
        text: &str,
        cx: &mut Context<Today>,
    ) -> Entity<Input> {
        let mut views = self.views.borrow_mut();
        let input = views
            .entry(key.to_string())
            .or_insert_with(|| cx.new(|cx| Input::read_only_text(label, cx)));
        if input.read(cx).content.as_ref() != text {
            input.update(cx, |input, cx| input.set_text(text.to_string(), cx));
        }
        input.clone()
    }
    /// Forget every field: another task is on screen.
    pub(super) fn clear(&self) {
        self.views.borrow_mut().clear();
    }
    #[cfg(test)]
    pub(super) fn get(&self, key: &str) -> Option<Entity<Input>> {
        self.views.borrow().get(key).cloned()
    }
}
