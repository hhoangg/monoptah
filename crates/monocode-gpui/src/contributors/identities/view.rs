//! The Identities entity: what the section remembers between frames (name
//! fields, the open menu, the drag, the focus to hand over once an edit lands)
//! and what each action does. Drawing is in `draw.rs`.
use super::{
    super::github::GithubView, Suggestion, add_person, can_remove, changed, dismiss,
    drag::DragState, excludable, exclude_identity_of, exclude_identity_text, exclude_key,
    exclude_person_of, exclude_person_text, include_again, merge_into, move_identity, remove,
    rename, suggestions,
};
use crate::{
    input::{self, Input},
    popover::{AnchorSlot, Menu, MenuAction, MenuEvent, MenuItem},
    theme::*,
};
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, Subscription, WeakEntity, Window,
};
use monocode_core::contributors::model::{
    pipeline::ContributorsModel,
    types::ContributorsConfig,
    unmapped::{ExcludedIdentity, PersonRow, excluded_identities, person_rows},
};
use std::{cell::Cell, collections::HashMap, rc::Rc, sync::Arc};

/// What the section shows, derived from one model. The scan of the commits
/// behind it runs once per model, not once per frame.
pub(super) struct Derived {
    pub model: Arc<ContributorsModel>,
    pub rows: Vec<PersonRow>,
    pub excluded: Vec<ExcludedIdentity>,
    pub suggestions: Vec<Suggestion>,
}

/// One frame's view of the world: the model on screen, the latest config (the
/// optimistic one while a save is in flight) and the lock.
#[derive(Clone)]
pub(super) struct Data {
    pub derived: Rc<Derived>,
    pub config: Arc<ContributorsConfig>,
    /// The config file cannot be read: nothing may be written.
    pub locked: bool,
    /// The model was built from the latest config, so a row that is not there is not coming.
    pub current: bool,
}
impl Data {
    pub fn merging(&self) -> bool {
        self.derived.model.merge
    }
    pub fn editable(&self) -> bool {
        self.merging() && !self.locked
    }
    pub fn row(&self, id: &str) -> Option<&PersonRow> {
        self.derived.rows.iter().find(|row| row.id == id)
    }
}

pub(super) struct NameField {
    pub input: Entity<Input>,
    /// The name the model showed when the field last followed it. The field
    /// only follows a change of the model, so a name just typed and saved does
    /// not flicker back while the new model is built.
    pub seen: String,
    /// The user is in the field or left it and the blur has not committed yet:
    /// the text is a draft, not a name that was refused.
    pub editing: bool,
    _subscriptions: Vec<Subscription>,
}

/// Which trigger opened the menu: what a picked item means.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MenuKind {
    Person(String),
    Identity { person: String, key: String },
    Move { key: String },
    Add { key: String },
}

pub(super) struct OpenMenu {
    pub menu: Entity<Menu>,
    pub anchor: AnchorSlot,
    pub kind: MenuKind,
    _events: Subscription,
}

pub(crate) struct IdentitiesView {
    pub(super) github: WeakEntity<GithubView>,
    pub(super) focus: FocusHandle,
    pub(super) names: HashMap<String, NameField>,
    slots: HashMap<String, AnchorSlot>,
    pub(super) row_focus: HashMap<String, FocusHandle>,
    pub(super) menu: Option<OpenMenu>,
    pub(super) drag: DragState,
    pub(super) show_excluded: bool,
    /// The person whose name takes the focus once its card is there.
    pub(super) pending_name: Option<String>,
    /// The identity row (key, person) that takes the focus once it is in that card.
    pub(super) pending_row: Option<(String, String)>,
    derived: Option<Rc<Derived>>,
    /// The page is wide enough for two columns at the last paint.
    pub(super) wide: Rc<Cell<bool>>,
}

impl IdentitiesView {
    pub(super) fn new(github: WeakEntity<GithubView>, cx: &mut Context<Self>) -> Self {
        Self {
            github,
            focus: cx.focus_handle(),
            names: HashMap::new(),
            slots: HashMap::new(),
            row_focus: HashMap::new(),
            menu: None,
            drag: DragState::default(),
            show_excluded: false,
            pending_name: None,
            pending_row: None,
            derived: None,
            wide: Rc::new(Cell::new(true)),
        }
    }

    /// The anchor a menu opens against: one per trigger, kept between frames.
    pub(super) fn slot(&mut self, id: &str) -> AnchorSlot {
        self.slots.entry(id.to_string()).or_default().clone()
    }

    /// The model, config and lock of the GithubView; `None` while there is no model.
    pub(super) fn data(&mut self, cx: &App) -> Option<Data> {
        let github = self.github.upgrade()?;
        let snap = github.read(cx).snapshot(cx);
        let model = snap.model?;
        let fresh = self
            .derived
            .as_ref()
            .is_some_and(|d| Arc::ptr_eq(&d.model, &model));
        if !fresh {
            let rows = person_rows(&model.raw, &model.config, &model.resolver);
            let derived = Derived {
                excluded: excluded_identities(&model.raw, &model.config),
                suggestions: suggestions(&model, &rows),
                rows,
                model: model.clone(),
            };
            self.derived = Some(Rc::new(derived));
        }
        Some(Data {
            derived: self.derived.clone()?,
            current: Arc::ptr_eq(&model.config, &snap.config) || *model.config == *snap.config,
            config: snap.config,
            locked: snap.config_error.is_some(),
        })
    }

    // ---- saving ----------------------------------------------------------

    /// Hands a changed config to the save queue; false when there is nothing
    /// to save or the file is locked. A failed write is reported by the page.
    pub(super) fn save(
        &mut self,
        next: Option<ContributorsConfig>,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(next), Some(github)) = (next, self.github.upgrade()) else {
            return false;
        };
        // The view's queue refuses the write too; this keeps the rule in both places.
        if self.data(cx).is_none_or(|d| d.locked) {
            return false;
        }
        github.update(cx, |github, cx| github.save_config(next, cx).is_ok())
    }

    pub(super) fn add_person(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        if !d.editable() {
            return;
        }
        let (next, id) = add_person(&d.config);
        self.pending_name = Some(id);
        self.save(changed(&d.config, next), cx);
    }

    /// Moves an identity to a person. `focus` hands the keyboard to the moved
    /// row once it is in its new card (the menu paths; a drag does not).
    pub(super) fn move_identity(
        &mut self,
        key: &str,
        target: &str,
        focus: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self.data(cx) else { return };
        let Some(target) = d.row(target) else { return };
        let next = move_identity(&d.config, &d.derived.rows, key, target);
        if focus && next.is_some() {
            self.pending_row = Some((key.to_string(), target.id.clone()));
        }
        if !self.save(next, cx) {
            self.pending_row = None;
        }
    }

    pub(super) fn merge_into(&mut self, target: &str, others: &[String], cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        let Some(target) = d.row(target) else { return };
        let others: Vec<PersonRow> = others.iter().filter_map(|id| d.row(id).cloned()).collect();
        self.pending_name = Some(target.id.clone());
        let next = merge_into(&d.config, target, &others);
        self.save(changed(&d.config, next), cx);
    }

    pub(super) fn merge_suggestion(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        let Some(item) = d.derived.suggestions.iter().find(|s| s.suggestion.id == id) else {
            return;
        };
        let Some((target, others)) = item.people.split_first() else {
            return;
        };
        let next = merge_into(&d.config, target, others);
        self.pending_name = Some(target.id.clone());
        self.save(changed(&d.config, next), cx);
    }

    pub(super) fn dismiss(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        self.save(dismiss(&d.config, id), cx);
    }

    pub(super) fn remove_person(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        if let Some(row) = d.row(id) {
            self.save(remove(&d.config, row), cx);
        }
    }

    pub(super) fn exclude_identity(&mut self, person: &str, key: &str, cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        if let Some(row) = d.row(person) {
            self.save(exclude_identity_of(&d.config, row, key), cx);
        }
    }

    pub(super) fn exclude_person(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        if let Some(row) = d.row(id) {
            self.save(exclude_person_of(&d.config, row), cx);
        }
    }

    pub(super) fn exclude_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        self.save(exclude_key(&d.config, key), cx);
    }

    pub(super) fn include(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(d) = self.data(cx) else { return };
        let Some(item) = d.derived.excluded.iter().find(|e| e.key == key) else {
            return;
        };
        let next = include_again(&d.config, item);
        self.save(changed(&d.config, next), cx);
    }

    // ---- the name fields ---------------------------------------------------------

    /// Makes a field per person and lets each follow a change of its name.
    pub(super) fn sync_names(&mut self, d: &Data, window: &mut Window, cx: &mut Context<Self>) {
        let rows = &d.derived.rows;
        self.names
            .retain(|id, _| rows.iter().any(|row| row.id == *id));
        for row in rows {
            if let Some(field) = self.names.get_mut(&row.id) {
                // A name that was never saved (the save failed or was refused)
                // does not stay in a field nobody is typing in. While a save is
                // in flight the model is not current yet, and the typed name stays.
                let stale = !field.editing
                    && d.current
                    && field.input.read(cx).content.as_ref() != row.name;
                if field.seen != row.name || stale {
                    field.seen = row.name.clone();
                    let name = row.name.clone();
                    field.input.update(cx, |input, cx| input.set_text(name, cx));
                }
                continue;
            }
            let input = cx.new(|cx| {
                let mut input = Input::field(
                    format!("Display name of {}", row.name),
                    "",
                    content(0.35),
                    cx,
                );
                input.set_text(row.name.clone(), cx);
                input
            });
            let focus = input.read(cx).focus_handle(cx);
            let id = row.id.clone();
            let subscriptions = vec![
                // Enter leaves the field, and leaving commits.
                cx.subscribe_in(
                    &input,
                    window,
                    |this, _, _: &input::Submitted, window, cx| {
                        window.focus(&this.focus, cx);
                    },
                ),
                cx.on_focus(&focus, window, {
                    let id = id.clone();
                    move |this, _, _| this.set_editing(&id, true)
                }),
                cx.on_blur(&focus, window, move |this, _, cx| this.commit_name(&id, cx)),
            ];
            self.names.insert(
                row.id.clone(),
                NameField {
                    input,
                    seen: row.name.clone(),
                    editing: false,
                    _subscriptions: subscriptions,
                },
            );
        }
    }

    fn set_editing(&mut self, id: &str, editing: bool) {
        if let Some(field) = self.names.get_mut(id) {
            field.editing = editing;
        }
    }

    /// Blur commits the draft: a blank one reverts, a changed one renames.
    fn commit_name(&mut self, id: &str, cx: &mut Context<Self>) {
        self.set_editing(id, false);
        let Some(input) = self.names.get(id).map(|f| f.input.clone()) else {
            return;
        };
        let draft = input.read(cx).content.to_string();
        let Some(d) = self.data(cx) else { return };
        let Some(row) = d.row(id) else { return };
        if draft.trim().is_empty() {
            let name = row.name.clone();
            input.update(cx, |input, cx| input.set_text(name, cx));
            return;
        }
        let name = row.name.clone();
        let next = rename(&d.config, row, &draft);
        // Nothing to save or a refused save: the draft goes back to the saved name.
        if !self.save(next, cx) {
            input.update(cx, |input, cx| input.set_text(name, cx));
        }
    }

    /// Escape in a name field: the draft goes back to the saved name and the
    /// field is left without saving (the blur then finds nothing to commit).
    pub(super) fn revert_name(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.names.get(id).map(|f| f.input.clone()) else {
            return;
        };
        if let Some(d) = self.data(cx)
            && let Some(row) = d.row(id)
        {
            let name = row.name.clone();
            input.update(cx, |input, cx| input.set_text(name, cx));
        }
        window.focus(&self.focus, cx);
    }

    /// Hands the focus to the name or the identity row an edit was waiting for,
    /// once it is on screen; drops the wish when the model caught up without it.
    pub(super) fn deliver_focus(&mut self, d: &Data, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.pending_name.clone() {
            match self.names.get(&id).map(|f| f.input.clone()) {
                Some(input) if d.editable() => {
                    window.focus(&input.read(cx).focus_handle(cx), cx);
                    input.update(cx, |input, cx| {
                        let end = input.content.len();
                        input.select_range(0..end, cx);
                    });
                    self.pending_name = None;
                }
                _ if d.current => self.pending_name = None,
                _ => {}
            }
        }
        if let Some((key, person)) = self.pending_row.clone() {
            let there = d
                .row(&person)
                .is_some_and(|row| row.identities.iter().any(|i| i.key == key));
            if there && let Some(handle) = self.row_focus.get(&key) {
                window.focus(handle, cx);
                self.pending_row = None;
            } else if !there && d.current {
                self.pending_row = None;
            }
        }
    }

    // ---- menus ----------------------------------------------------------------

    pub(super) fn open_menu(
        &mut self,
        kind: MenuKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self.data(cx) else { return };
        if !d.editable() {
            return;
        }
        let Some((label, items)) = menu_of(&kind, &d) else {
            return;
        };
        let anchor = self.slot(&anchor_id(&kind));
        let menu = cx.new(|cx| Menu::new(label, items, window, cx).width(240.));
        let events = cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            let kind = this.menu.take().map(|open| open.kind);
            if let (Some(kind), MenuEvent::Picked(id, _)) = (kind, event) {
                this.picked(&kind, id, cx);
            }
            cx.notify();
        });
        self.menu = Some(OpenMenu {
            menu,
            anchor,
            kind,
            _events: events,
        });
        cx.notify();
    }

    fn picked(&mut self, kind: &MenuKind, id: &str, cx: &mut Context<Self>) {
        match kind {
            MenuKind::Person(person) => {
                if let Some(target) = id.strip_prefix("merge:") {
                    self.merge_into(target, std::slice::from_ref(person), cx);
                } else if id == "exclude-person" {
                    self.exclude_person(person, cx);
                } else if id == "remove-person" {
                    self.remove_person(person, cx);
                }
            }
            MenuKind::Identity { person, key } => {
                if id == "exclude-identity" {
                    self.exclude_identity(person, key, cx);
                }
            }
            MenuKind::Move { key } | MenuKind::Add { key } => {
                if let Some(target) = id.strip_prefix("move:").or_else(|| id.strip_prefix("add:")) {
                    self.move_identity(key, target, true, cx);
                }
            }
        }
    }

    // ---- the drag ---------------------------------------------------------------

    /// A release over this person's card.
    pub(super) fn dropped(&mut self, person: &str, cx: &mut Context<Self>) {
        if let Some((key, target)) = self.drag.drop_on(person) {
            self.move_identity(&key, &target, false, cx);
        }
        cx.notify();
    }

    /// Escape during a drag ends it without a drop.
    pub(super) fn cancel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drag.cancel();
        cx.stop_active_drag(window);
        cx.notify();
    }
}

/// The key of the anchor slot of the trigger that opens this menu.
pub(super) fn anchor_id(kind: &MenuKind) -> String {
    match kind {
        MenuKind::Person(id) => format!("person:{id}"),
        MenuKind::Identity { key, .. } => format!("identity:{key}"),
        MenuKind::Move { key } => format!("move:{key}"),
        MenuKind::Add { key } => format!("add:{key}"),
    }
}

/// People as menu rows: `prefix` + id.
fn people_actions(prefix: &str, people: &[&PersonRow]) -> Vec<MenuAction> {
    people
        .iter()
        .map(|p| MenuAction::new(format!("{prefix}{}", p.id), p.name.clone()))
        .collect()
}

/// The aria label and the rows of the menu a trigger opens.
pub(super) fn menu_of(kind: &MenuKind, d: &Data) -> Option<(String, Vec<MenuItem>)> {
    let rows = &d.derived.rows;
    match kind {
        MenuKind::Person(id) => {
            let row = d.row(id)?;
            let others: Vec<&PersonRow> = rows.iter().filter(|p| p.id != row.id).collect();
            let none = excludable(row) == 0;
            Some((
                format!("Actions for {}", row.name),
                vec![
                    MenuAction::new("merge", "Merge into...")
                        .disabled(others.is_empty())
                        .submenu(people_actions("merge:", &others))
                        .into(),
                    MenuAction::new("exclude-person", "Exclude person")
                        .danger()
                        .disabled(none)
                        .description(if none {
                            "Agents cannot be excluded.".to_string()
                        } else {
                            exclude_person_text(row)
                        })
                        .into(),
                    MenuAction::new("remove-person", "Remove person")
                        .danger()
                        .disabled(!can_remove(row))
                        .into(),
                ],
            ))
        }
        MenuKind::Identity { person, key } => {
            let agent = d
                .row(person)?
                .identities
                .iter()
                .any(|i| i.key == *key && i.agent);
            Some((
                format!("Actions for identity {key}"),
                vec![
                    MenuAction::new("exclude-identity", "Exclude identity")
                        .danger()
                        .disabled(agent)
                        .description(exclude_identity_text(agent))
                        .into(),
                ],
            ))
        }
        MenuKind::Move { key } => {
            let owner = rows
                .iter()
                .find(|row| row.identities.iter().any(|i| i.key == *key));
            let others: Vec<&PersonRow> = rows
                .iter()
                .filter(|p| Some(p.id.as_str()) != owner.map(|o| o.id.as_str()))
                .collect();
            Some(("Move to".into(), people_menu("move:", &others)))
        }
        MenuKind::Add { key } => {
            // The unmapped key itself is never a target (as in the TS list).
            let targets: Vec<&PersonRow> = rows.iter().filter(|p| p.id != *key).collect();
            Some(("Add to person".into(), people_menu("add:", &targets)))
        }
    }
}

fn people_menu(prefix: &str, people: &[&PersonRow]) -> Vec<MenuItem> {
    if people.is_empty() {
        return vec![MenuItem::Note("No other people".into())];
    }
    people_actions(prefix, people)
        .into_iter()
        .map(MenuItem::from)
        .collect()
}

/// What the window tests look at.
#[cfg(test)]
impl IdentitiesView {
    pub(in crate::contributors) fn name_input(&self, id: &str) -> Option<Entity<Input>> {
        self.names.get(id).map(|field| field.input.clone())
    }
    pub(in crate::contributors) fn row_handle(&self, key: &str) -> Option<FocusHandle> {
        self.row_focus.get(key).cloned()
    }
    pub(in crate::contributors) fn root_focus(&self) -> FocusHandle {
        self.focus.clone()
    }
    /// A focus wish still waiting for its target.
    pub(in crate::contributors) fn focus_pending(&self) -> bool {
        self.pending_name.is_some() || self.pending_row.is_some()
    }
    pub(in crate::contributors) fn drag_over(&self) -> Option<String> {
        self.drag.over().map(String::from)
    }
    pub(in crate::contributors) fn dragging(&self) -> bool {
        self.drag.is_active()
    }
    pub(in crate::contributors) fn menu_is_open(&self) -> bool {
        self.menu.is_some()
    }
}
