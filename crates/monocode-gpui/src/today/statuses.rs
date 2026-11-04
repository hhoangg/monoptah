//! The Statuses and Tags dialogs: one implementation, two kinds of list.
//!
//! Statuses are grouped by stage and may be built in; tags are one flat list
//! with no built ins. Both are renamed, recolored, reordered, added and deleted
//! the same way, so the dialog state, row renderer, swatches, color picker,
//! name commit and serialized color writes are shared and only the few places
//! that differ branch on [`Kind`].
//!
//! The dialog keeps only view state (one name input per item, the color
//! picker that is open, the focus); the list itself stays `Today::statuses` or
//! `Today::tags` and every edit goes through the store, which refreshes the page.
use super::{
    Today,
    list::{color_dot, drag_ghost},
};
use crate::{
    inbox::{action, icon_action, message},
    input::{self, Input},
    modal,
    monos::color_picker::{ColorPicker, Picked, spectrum},
    theme::*,
};
use gpui::{
    AnyElement, Context, CursorStyle, Entity, FocusHandle, Focusable, FontWeight, KeyDownEvent,
    Render, Role, SharedString, Subscription, Window, div, prelude::*, px, rgb,
};
use monocode_core::today::tags::same_name;
use monocode_core::today::{
    Tag, TagPatch, WorkflowStatus, statuses as status_rules,
    statuses::{Stage, StatusPatch, format_color, presets},
    tags as tag_rules,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

/// Width of every name input, so the swatches line up down the dialog.
const NAME_WIDTH: f32 = 200.;
/// The last cell of a row: "Built in" or the delete action, same width.
const ACTION_WIDTH: f32 = 84.;
const SWATCH: f32 = 18.;
const CARD_WIDTH: f32 = 640.;

/// Which list a dialog edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Statuses,
    Tags,
}
impl Kind {
    /// The start of every selector of a row: `status-row-<id>`, `tag-row-<id>`.
    fn prefix(self) -> &'static str {
        match self {
            Kind::Statuses => "status",
            Kind::Tags => "tag",
        }
    }
    /// The dialog's own name: overlay, `statuses-done`, `tags-error`.
    fn plural(self) -> &'static str {
        match self {
            Kind::Statuses => "statuses",
            Kind::Tags => "tags",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Kind::Statuses => "Statuses",
            Kind::Tags => "Tags",
        }
    }
    fn body(self) -> &'static str {
        match self {
            Kind::Statuses => {
                "Board columns and list groups follow this order. The stage decides \
                 what syncing treats as finished. Drag a status to reorder it or \
                 move it to another stage. Built in statuses can be renamed and \
                 recolored."
            }
            Kind::Tags => {
                "Tags label tasks. Rename or recolor one and it changes everywhere. \
                 Drag a tag to reorder it. Deleting a tag removes it from every task."
            }
        }
    }
    fn field_label(self) -> &'static str {
        match self {
            Kind::Statuses => "Status name",
            Kind::Tags => "Tag name",
        }
    }
    fn taken_error(self) -> &'static str {
        match self {
            Kind::Statuses => "A status with that name already exists",
            Kind::Tags => "A tag with that name already exists",
        }
    }
    fn max_name(self) -> usize {
        match self {
            Kind::Statuses => status_rules::MAX_NAME,
            Kind::Tags => tag_rules::MAX_NAME,
        }
    }
    fn base_name(self) -> &'static str {
        match self {
            Kind::Statuses => "New status",
            Kind::Tags => "New tag",
        }
    }
}

/// One editable entry, whichever list it comes from.
struct Item {
    id: String,
    name: String,
    color: u32,
    builtin: bool,
    /// The stage of a status; `None` for a tag.
    group: Option<Stage>,
    presets: Vec<u32>,
}

/// The first of `base`, `base 2`, `base 3`... that no name takes (ignoring case).
fn first_free(base: &str, names: &[&str]) -> String {
    let taken = |name: &str| names.iter().any(|n| same_name(n, name));
    let mut name = base.to_owned();
    let mut n = 2;
    while taken(&name) {
        name = format!("{base} {n}");
        n += 1;
    }
    name
}
/// The suggested name for a new status: "New status", "New status 2", ...
fn default_name(statuses: &[WorkflowStatus]) -> String {
    let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
    first_free(Kind::Statuses.base_name(), &names)
}
/// The suggested name for a new tag: "New tag", "New tag 2", ...
fn default_tag_name(tags: &[Tag]) -> String {
    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    first_free(Kind::Tags.base_name(), &names)
}

/// Where a dragged status lands: the stage and the index inside it that
/// `Store::today_move_status` takes. `before` is the row it was dropped on
/// (it takes that row's place); `None` is the end of the stage. A built-in
/// never leaves its stage, and a status dropped on itself does not move.
fn drop_target(
    statuses: &[WorkflowStatus],
    id: &str,
    stage: Stage,
    before: Option<&str>,
) -> Option<(Stage, usize)> {
    let moved = statuses.iter().find(|s| s.id == id)?;
    if moved.builtin && moved.stage != stage || before == Some(id) {
        return None;
    }
    let in_stage: Vec<&WorkflowStatus> = statuses.iter().filter(|s| s.stage == stage).collect();
    let index = match before {
        Some(target) => in_stage.iter().position(|s| s.id == target)?,
        None => in_stage.len(),
    };
    Some((stage, index))
}
/// Where a dragged tag lands: the index `Store::today_move_tag` takes. Dropped
/// on a row it takes that row's place; `None` is the end of the list. A tag
/// dropped on itself, or an unknown one, does not move.
fn tag_drop_index(tags: &[Tag], id: &str, before: Option<&str>) -> Option<usize> {
    if before == Some(id) || !tags.iter().any(|t| t.id == id) {
        return None;
    }
    match before {
        Some(target) => tags.iter().position(|t| t.id == target),
        None => Some(tags.len()),
    }
}

/// What the delete confirmation says for a tag held by `count` tasks.
fn delete_prompt(count: usize) -> String {
    match count {
        0 => "Not used by any task. Delete it?".into(),
        1 => "Used by 1 task. Delete it and remove it from that task?".into(),
        n => format!("Used by {n} tasks. Delete it and remove it from them?"),
    }
}

fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::NotStarted => "not_started",
        Stage::Started => "started",
        Stage::Finished => "finished",
    }
}
fn stage_title(stage: Stage) -> &'static str {
    match stage {
        Stage::NotStarted => "NOT STARTED",
        Stage::Started => "STARTED",
        Stage::Finished => "FINISHED",
    }
}
fn stage_hint(stage: Stage) -> &'static str {
    match stage {
        Stage::NotStarted => "Waiting to begin",
        Stage::Started => "Being worked on",
        Stage::Finished => "Counts as done when syncing",
    }
}

/// What travels with the pointer while a row handle is dragged.
#[derive(Clone)]
struct DraggedItem {
    kind: Kind,
    id: String,
    name: SharedString,
    color: u32,
    /// The stage a built-in is tied to; it cannot be dropped anywhere else.
    builtin_stage: Option<Stage>,
}
impl Render for DraggedItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_ghost()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .child(color_dot(self.color))
            .child(self.name.clone())
    }
}

/// The newest color picked and not yet written, with its item.
#[derive(Default)]
struct ColorQueue {
    pending: Option<(String, u32)>,
    /// A write is in flight; the next one waits for it.
    writing: bool,
}
static NEXT_DIALOG: AtomicU64 = AtomicU64::new(1);

struct RowInput {
    id: String,
    input: Entity<Input>,
    _submitted: Subscription,
    _edited: Subscription,
    /// The name last sent to the store, so Enter then blur saves once.
    committed: Option<String>,
    /// Committing on blur needs the window, so it is wired on first render.
    blur: Option<Subscription>,
}
struct OpenPicker {
    id: String,
    view: Entity<ColorPicker>,
    _picked: Subscription,
}

/// The open dialog's state (name inputs, color being picked).
pub(crate) struct Dialog {
    kind: Kind,
    rows: Vec<RowInput>,
    picker: Option<OpenPicker>,
    focus: FocusHandle,
    focused: bool,
    /// Color writes. Shared with the writing task, so a write in flight (and
    /// the newest pick queued behind it) still lands after the dialog closes.
    colors: Rc<RefCell<ColorQueue>>,
    /// Tells this dialog from an earlier one that was closed and reopened.
    id: u64,
    /// A tag whose delete is awaiting an answer, with how many tasks hold it.
    confirm: Option<(String, usize)>,
}
impl Dialog {
    fn new(kind: Kind, items: &[Item], cx: &mut Context<Today>) -> Self {
        Self {
            kind,
            rows: items.iter().map(|i| Self::row(kind, i, cx)).collect(),
            picker: None,
            focus: cx.focus_handle(),
            focused: false,
            colors: Rc::default(),
            id: NEXT_DIALOG.fetch_add(1, Ordering::Relaxed),
            confirm: None,
        }
    }
    fn row(kind: Kind, item: &Item, cx: &mut Context<Today>) -> RowInput {
        let input = cx.new(|cx| {
            let mut field = Input::field(kind.field_label(), "Name", content(0.35), cx);
            field.set_text(item.name.clone(), cx);
            field
        });
        let id = item.id.clone();
        let submitted = cx.subscribe(
            &input,
            move |this: &mut Today, _, _: &input::Submitted, cx| this.commit_name(kind, &id, cx),
        );
        let id = item.id.clone();
        let edited = cx.subscribe(&input, move |this: &mut Today, _, _: &input::Changed, _| {
            if let Some(row) = this
                .dialog_mut(kind)
                .and_then(|d| d.rows.iter_mut().find(|row| row.id == id))
            {
                row.committed = None;
            }
        });
        RowInput {
            id: item.id.clone(),
            input,
            _submitted: submitted,
            _edited: edited,
            committed: None,
            blur: None,
        }
    }
    pub(super) fn input(&self, id: &str) -> Option<Entity<Input>> {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.input.clone())
    }
    #[cfg(test)]
    pub(super) fn picker_view(&self) -> Option<Entity<ColorPicker>> {
        self.picker.as_ref().map(|p| p.view.clone())
    }
    /// Brings the inputs in step with the list: new rows get one, removed rows
    /// drop theirs, and a name that is not being edited follows a rename made
    /// elsewhere. Also takes focus on the first call so Escape reaches the dialog.
    fn sync(&mut self, items: &[Item], window: &mut Window, cx: &mut Context<Today>) {
        let kind = self.kind;
        self.rows.retain(|row| items.iter().any(|i| i.id == row.id));
        if self
            .picker
            .as_ref()
            .is_some_and(|p| !items.iter().any(|i| i.id == p.id))
        {
            self.picker = None;
        }
        if self
            .confirm
            .as_ref()
            .is_some_and(|(id, _)| !items.iter().any(|i| i.id == *id))
        {
            self.confirm = None;
        }
        for item in items {
            if !self.rows.iter().any(|row| row.id == item.id) {
                self.rows.push(Self::row(kind, item, cx));
            }
        }
        for row in &mut self.rows {
            let Some(item) = items.iter().find(|i| i.id == row.id) else {
                continue;
            };
            let focus = row.input.read(cx).focus_handle(cx);
            if !focus.is_focused(window) && row.input.read(cx).content != item.name {
                let name = item.name.clone();
                row.input.update(cx, |field, cx| field.set_text(name, cx));
            }
            if row.blur.is_none() {
                let id = row.id.clone();
                row.blur = Some(cx.on_focus_out(&focus, window, move |this, _, _, cx| {
                    this.commit_name(kind, &id, cx)
                }));
            }
        }
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
    }
}

impl Today {
    /// A fresh dialog of `kind` over the current list.
    pub(super) fn new_dialog(&self, kind: Kind, cx: &mut Context<Self>) -> Dialog {
        Dialog::new(kind, &self.items(kind), cx)
    }
    fn items(&self, kind: Kind) -> Vec<Item> {
        match kind {
            Kind::Statuses => self
                .statuses
                .iter()
                .map(|s| Item {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    color: s.rgb(),
                    builtin: s.builtin,
                    group: Some(s.stage),
                    presets: presets(s.stage).to_vec(),
                })
                .collect(),
            Kind::Tags => self
                .tags
                .iter()
                .map(|t| Item {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    color: t.rgb(),
                    builtin: false,
                    group: None,
                    presets: tag_rules::PRESETS.to_vec(),
                })
                .collect(),
        }
    }
    fn dialog(&self, kind: Kind) -> Option<&Dialog> {
        match kind {
            Kind::Statuses => self.statuses_dialog.as_ref(),
            Kind::Tags => self.tags_dialog.as_ref(),
        }
    }
    fn dialog_mut(&mut self, kind: Kind) -> Option<&mut Dialog> {
        match kind {
            Kind::Statuses => self.statuses_dialog.as_mut(),
            Kind::Tags => self.tags_dialog.as_mut(),
        }
    }

    /// Saves the name typed in an item's input, or puts the old one back when
    /// it is empty or already taken. A name that did not change does nothing.
    fn commit_name(&mut self, kind: Kind, id: &str, cx: &mut Context<Self>) {
        let Some(input) = self.dialog(kind).and_then(|d| d.input(id)) else {
            return;
        };
        let items = self.items(kind);
        let Some(current) = items.iter().find(|i| i.id == id).map(|i| i.name.clone()) else {
            return;
        };
        let typed = input.read(cx).content.trim().to_owned();
        let already = self
            .dialog(kind)
            .and_then(|d| d.rows.iter().find(|row| row.id == id))
            .is_some_and(|row| row.committed.as_deref() == Some(typed.as_str()));
        if typed == current || already {
            return;
        }
        let name: String = typed.chars().take(kind.max_name()).collect();
        let taken = items
            .iter()
            .any(|i| i.id != id && tag_rules::same_name(&i.name, &name));
        if typed.is_empty() || taken {
            if taken {
                self.action_error = Some(kind.taken_error().into());
            }
            input.update(cx, |field, cx| field.set_text(current, cx));
            cx.notify();
            return;
        }
        if let Some(row) = self
            .dialog_mut(kind)
            .and_then(|d| d.rows.iter_mut().find(|row| row.id == id))
        {
            row.committed = Some(typed);
        }
        match kind {
            Kind::Statuses => self.update_status(
                id.to_owned(),
                StatusPatch {
                    name: Some(name),
                    color: None,
                },
                cx,
            ),
            Kind::Tags => self.update_tag(
                id.to_owned(),
                TagPatch {
                    name: Some(name),
                    color: None,
                },
                cx,
            ),
        }
    }
    fn commit_all_names(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.items(kind).into_iter().map(|i| i.id).collect();
        for id in ids {
            self.commit_name(kind, &id, cx);
        }
    }
    /// Recolors an item from a swatch or the picker. The dot changes at once;
    /// the writes are serialized so the stored color is always the last one picked.
    fn set_color(&mut self, kind: Kind, id: &str, color: u32, cx: &mut Context<Self>) {
        match kind {
            Kind::Statuses => {
                if let Some(status) = self.statuses.iter_mut().find(|s| s.id == id) {
                    status.color = format_color(color);
                }
            }
            Kind::Tags => {
                if let Some(tag) = self.tags.iter_mut().find(|t| t.id == id) {
                    tag.color = format_color(color);
                }
            }
        }
        if let Some(dialog) = self.dialog(kind) {
            dialog.colors.borrow_mut().pending = Some((id.to_owned(), color));
        }
        cx.notify();
        self.flush_color(kind, cx);
    }
    fn flush_color(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let Some(queue) = self.dialog(kind).map(|d| d.colors.clone()) else {
            return;
        };
        let next = {
            let mut queue = queue.borrow_mut();
            if queue.writing {
                return;
            }
            let Some(next) = queue.pending.take() else {
                return;
            };
            queue.writing = true;
            next
        };
        self.write_color(kind, queue, next, cx);
    }
    /// Writes one color, then the newest queued behind it, until none is left;
    /// then refreshes. It holds the queue itself, so it does not matter whether
    /// the dialog is still open, or was reopened, meanwhile.
    fn write_color(
        &mut self,
        kind: Kind,
        queue: Rc<RefCell<ColorQueue>>,
        (id, color): (String, u32),
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let color = format_color(color);
        let write = cx.background_executor().spawn(async move {
            let db = super::locked(&store)?;
            match kind {
                Kind::Statuses => {
                    let patch = StatusPatch {
                        name: None,
                        color: Some(color),
                    };
                    db.today_update_status(&id, &patch, super::now_ms())
                        .map(|_| ())
                }
                Kind::Tags => {
                    let patch = TagPatch {
                        name: None,
                        color: Some(color),
                    };
                    db.today_update_tag(&id, &patch).map(|_| ())
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = write.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.action_error = Some(error);
                }
                let more = {
                    let mut queue = queue.borrow_mut();
                    let more = queue.pending.take();
                    queue.writing = more.is_some();
                    more
                };
                match more {
                    Some(next) => this.write_color(kind, queue, next, cx),
                    None => this.refresh(cx),
                }
            });
        })
        .detach();
    }
    /// Saves every name still being typed, closes the dialog and hands the
    /// focus back to the page.
    fn finish(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_all_names(kind, cx);
        match kind {
            Kind::Statuses => self.close_statuses(cx),
            Kind::Tags => self.close_tags(cx),
        }
        window.focus(&self.focus, cx);
    }
    pub(super) fn finish_statuses(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish(Kind::Statuses, window, cx)
    }
    pub(super) fn finish_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish(Kind::Tags, window, cx)
    }
    /// A status handle was dropped on a row (`before`) or on the end of a stage.
    /// A built-in dropped on another stage is refused without a word.
    pub(super) fn drop_status(
        &mut self,
        id: &str,
        stage: Stage,
        before: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some((stage, index)) = drop_target(&self.statuses, id, stage, before) {
            self.move_status(id.to_owned(), stage, index, cx);
        }
    }
    /// A tag handle was dropped on a row (`before`) or on the end of the list.
    pub(super) fn drop_tag(&mut self, id: &str, before: Option<&str>, cx: &mut Context<Self>) {
        if let Some(index) = tag_drop_index(&self.tags, id, before) {
            self.move_tag(id.to_owned(), index, cx);
        }
    }
    fn drop_item(
        &mut self,
        dragged: &DraggedItem,
        group: Option<Stage>,
        before: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        match (dragged.kind, group) {
            (Kind::Statuses, Some(stage)) => self.drop_status(&dragged.id, stage, before, cx),
            (Kind::Tags, _) => self.drop_tag(&dragged.id, before, cx),
            _ => {}
        }
    }
    /// "+ Add": a new item at the end of the group, with a free default name.
    fn add_item(&mut self, kind: Kind, group: Option<Stage>, cx: &mut Context<Self>) {
        match (kind, group) {
            (Kind::Statuses, Some(stage)) => {
                let name = default_name(&self.statuses);
                self.add_status(name, stage, format_color(presets(stage)[0]), cx)
            }
            (Kind::Tags, _) => {
                let name = default_tag_name(&self.tags);
                let color = format_color(tag_rules::preset_for(self.tags.len()));
                self.add_tag(name, color, cx)
            }
            _ => {}
        }
    }
    /// The delete icon of a row: a status goes at once, a tag asks first.
    fn delete_item(&mut self, kind: Kind, id: &str, cx: &mut Context<Self>) {
        match kind {
            Kind::Statuses => self.delete_status(id.to_owned(), cx),
            Kind::Tags => self.ask_delete_tag(id, cx),
        }
    }
    /// Counts the tasks holding a tag off the UI thread, then asks inside the row.
    fn ask_delete_tag(&mut self, id: &str, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let target = id.to_owned();
        let dialog = self.tags_dialog.as_ref().map(|d| d.id);
        let count = {
            let target = target.clone();
            cx.background_executor()
                .spawn(async move { super::locked(&store)?.today_tag_usage(&target) })
        };
        cx.spawn(async move |this, cx| {
            let result = count.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(count) => {
                    // A dialog closed and reopened meanwhile asked nothing.
                    if let Some(dialog) = this.tags_dialog.as_mut().filter(|d| Some(d.id) == dialog)
                    {
                        dialog.confirm = Some((target, count));
                        cx.notify();
                    }
                }
                Err(error) => {
                    this.action_error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn cancel_delete_tag(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.tags_dialog.as_mut() {
            dialog.confirm = None;
        }
        cx.notify();
    }
    fn confirm_delete_tag(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.tags_dialog.as_mut() {
            dialog.confirm = None;
        }
        self.delete_tag(id.to_owned(), cx);
    }
    /// Opens the color picker under a row, or closes it when it is that row's.
    fn toggle_picker(&mut self, kind: Kind, id: &str, cx: &mut Context<Self>) {
        let Some(color) = self
            .items(kind)
            .into_iter()
            .find(|i| i.id == id)
            .map(|i| i.color)
        else {
            return;
        };
        let Some(dialog) = self.dialog_mut(kind) else {
            return;
        };
        if dialog.picker.as_ref().is_some_and(|p| p.id == id) {
            dialog.picker = None;
        } else {
            let view = cx.new(|cx| ColorPicker::new(color, cx));
            let target = id.to_owned();
            let picked = cx.subscribe(&view, move |this: &mut Today, _, event: &Picked, cx| {
                this.set_color(kind, &target, event.0, cx)
            });
            if let Some(dialog) = self.dialog_mut(kind) {
                dialog.picker = Some(OpenPicker {
                    id: id.to_owned(),
                    view,
                    _picked: picked,
                });
            }
        }
        cx.notify();
    }

    /// The Statuses dialog over the page, while open.
    pub(super) fn statuses_dialog_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.dialog_view(Kind::Statuses, window, cx)
    }
    /// The Tags dialog over the page, while open.
    pub(super) fn tags_dialog_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.dialog_view(Kind::Tags, window, cx)
    }

    fn dialog_view(
        &mut self,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let items = self.items(kind);
        self.dialog_mut(kind)?.sync(&items, window, cx);
        let focus = self.dialog(kind)?.focus.clone();
        let sections = match kind {
            Kind::Statuses => Stage::ALL
                .into_iter()
                .map(|stage| self.section(kind, Some(stage), &items, cx))
                .collect::<Vec<_>>(),
            Kind::Tags => vec![self.section(kind, None, &items, cx)],
        };
        let height = f32::from(window.viewport_size().height);
        let error = self.action_error.clone();
        let name = kind.plural();
        let card = modal::card()
            .w(px(CARD_WIDTH))
            .track_focus(&focus)
            .key_context(match kind {
                Kind::Statuses => "StatusesDialog",
                Kind::Tags => "TagsDialog",
            })
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    // An open delete question closes first, then the dialog.
                    if this.dialog(kind).is_some_and(|d| d.confirm.is_some()) {
                        this.cancel_delete_tag(cx);
                    } else {
                        this.finish(kind, window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(
                modal::card_body()
                    .child(modal::title(kind.title()))
                    .child(
                        modal::body(kind.body())
                            .mt(px(6.))
                            .text_size(px(12.))
                            .line_height(px(18.)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{name}-sections")))
                            .mt(px(8.))
                            .max_h(px((height - 260.).max(160.)))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(sections),
                    )
                    .children(error.map(|text| {
                        message(text)
                            .debug_selector(move || format!("{name}-error"))
                            .px_0()
                            .text_color(danger_color())
                    }))
                    .child(
                        modal::actions().child(
                            modal::primary_button(format!("{name}-done"), "Done")
                                .debug_selector(move || format!("{name}-done"))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.finish(kind, window, cx)
                                })),
                        ),
                    ),
            );
        let this = cx.entity();
        Some(
            modal::window_overlay(
                window,
                modal::overlay(name, card, move |window, cx| {
                    this.update(cx, |today, cx| today.finish(kind, window, cx))
                }),
            )
            .into_any_element(),
        )
    }

    /// One group of rows: a stage of statuses, or the whole list of tags.
    fn section(
        &self,
        kind: Kind,
        group: Option<Stage>,
        items: &[Item],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = kind.plural();
        let key = group.map(stage_key);
        let section_id = match key {
            Some(key) => format!("{name}-section-{key}"),
            None => format!("{name}-section"),
        };
        let add_id = match key {
            Some(key) => format!("{}-add-{key}", kind.prefix()),
            None => format!("{}-add", kind.prefix()),
        };
        let open_picker = self
            .dialog(kind)
            .and_then(|d| d.picker.as_ref())
            .map(|p| (p.id.clone(), p.view.clone()));
        let confirm = self.dialog(kind).and_then(|d| d.confirm.clone());
        let body = items
            .iter()
            .filter(|i| i.group == group)
            .enumerate()
            .map(|(ix, item)| {
                let picker = open_picker
                    .as_ref()
                    .filter(|(id, _)| *id == item.id)
                    .map(|(_, view)| view.clone());
                let ask = confirm
                    .as_ref()
                    .filter(|(id, _)| *id == item.id)
                    .map(|(_, count)| *count);
                self.item_row(kind, item, ix > 0, picker, ask, cx)
            })
            .collect::<Vec<_>>();
        let mut header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mt(px(10.))
            .mb(px(4.));
        if let Some(stage) = group {
            header = header
                .child(
                    div()
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(content(0.4))
                        .child(stage_title(stage)),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(stage_hint(stage)),
                );
        }
        let selector = add_id.clone();
        div()
            .id(SharedString::from(section_id.clone()))
            .debug_selector(move || section_id.clone())
            .flex()
            .flex_col()
            .child(
                header.child(div().flex_1()).child(
                    action(add_id, "+ Add")
                        .debug_selector(move || selector.clone())
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.add_item(kind, group, cx)),
                        ),
                ),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "{name}-drop-{}",
                        key.unwrap_or("all")
                    )))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.03))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .drag_over::<DraggedItem>(move |style, dragged, _, _| {
                        if dragged.kind != kind
                            || dragged.builtin_stage.is_some_and(|own| Some(own) != group)
                        {
                            style
                        } else {
                            style.bg(accent_color().opacity(0.08))
                        }
                    })
                    .on_drop(cx.listener(move |this, dragged: &DraggedItem, _, cx| {
                        if dragged.kind == kind {
                            this.drop_item(dragged, group, None, cx)
                        }
                    }))
                    .children(body),
            )
            .into_any_element()
    }

    /// One row. `ask` is the usage count while a tag's delete awaits an answer.
    fn item_row(
        &self,
        kind: Kind,
        item: &Item,
        divided: bool,
        picker: Option<Entity<ColorPicker>>,
        ask: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = kind.prefix();
        let id = item.id.clone();
        let group = item.group;
        let color = item.color;
        let input = self.dialog(kind).and_then(|d| d.input(&id));
        let payload = DraggedItem {
            kind,
            id: id.clone(),
            name: item.name.clone().into(),
            color,
            builtin_stage: if item.builtin { item.group } else { None },
        };
        let handle = div()
            .id(SharedString::from(format!("{p}-grab-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("{p}-grab-{id}")
            })
            .w(px(20.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .cursor(CursorStyle::OpenHand)
            .child(icon("grip-vertical", 14.).text_color(content(0.3)))
            .on_drag(payload, |drag, _, _, cx| cx.new(|_| drag.clone()));
        let swatches = item.presets.iter().copied().enumerate().map(|(n, preset)| {
            let target = id.clone();
            let chosen = color == preset;
            swatch(format!("{p}-swatch-{id}-{n}"), chosen)
                .bg(rgb(preset))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.set_color(kind, &target, preset, cx)),
                )
        });
        let custom = {
            let target = id.clone();
            let chosen = !item.presets.contains(&color);
            swatch(format!("{p}-custom-{id}"), chosen)
                .relative()
                .overflow_hidden()
                .child(div().absolute().inset_0().child(spectrum()))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_picker(kind, &target, cx)))
        };
        let end = if item.builtin {
            div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("{p}-builtin-{id}")
                })
                .text_size(px(11.))
                .text_color(content(0.4))
                .child("Built in")
                .into_any_element()
        } else {
            let target = id.clone();
            icon_action(format!("{p}-delete-{id}"), "Delete", "trash")
                .debug_selector({
                    let id = id.clone();
                    move || format!("{p}-delete-{id}")
                })
                .on_click(cx.listener(move |this, _, _, cx| this.delete_item(kind, &target, cx)))
                .into_any_element()
        };
        let drop_id = id.clone();
        div()
            .id(SharedString::from(format!("{p}-row-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("{p}-row-{id}")
            })
            .flex()
            .flex_col()
            .when(divided, |row| row.border_t_1().border_color(content(0.08)))
            .drag_over::<DraggedItem>(move |style, dragged, _, _| {
                if dragged.kind != kind
                    || dragged.builtin_stage.is_some_and(|own| Some(own) != group)
                {
                    style
                } else {
                    style.bg(accent_color().opacity(0.12))
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedItem, _, cx| {
                cx.stop_propagation();
                if dragged.kind == kind {
                    this.drop_item(dragged, group, Some(&drop_id), cx)
                }
            }))
            .child(
                div()
                    .h(px(44.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(handle)
                    .child(color_dot(color))
                    .child(
                        div()
                            .debug_selector({
                                let id = id.clone();
                                move || format!("{p}-name-{id}")
                            })
                            .w(px(NAME_WIDTH))
                            .flex_shrink_0()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.05))
                            .text_size(px(13.))
                            .children(input),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .children(swatches)
                            .child(custom),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .w(px(ACTION_WIDTH))
                            .flex_shrink_0()
                            .flex()
                            .justify_end()
                            .items_center()
                            .child(end),
                    ),
            )
            .children(picker.map(|view| div().px(px(10.)).pb(px(10.)).child(view)))
            .children(ask.map(|count| self.delete_question(&id, count, cx)))
            .into_any_element()
    }

    /// The inline question under a tag row: how many tasks use it, then Cancel / Delete.
    fn delete_question(&self, id: &str, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let cancel = id.to_owned();
        let confirm = id.to_owned();
        let selector = format!("tag-delete-text-{id}");
        div()
            .px(px(10.))
            .pb(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .child(delete_prompt(count)),
            )
            .child(
                modal::ghost_button(format!("tag-delete-cancel-{cancel}"), "Cancel")
                    .debug_selector({
                        let id = cancel.clone();
                        move || format!("tag-delete-cancel-{id}")
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.cancel_delete_tag(cx))),
            )
            .child(
                modal::danger_button(format!("tag-delete-confirm-{confirm}"), "Delete")
                    .debug_selector({
                        let id = confirm.clone();
                        move || format!("tag-delete-confirm-{id}")
                    })
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.confirm_delete_tag(&confirm, cx)),
                    ),
            )
            .into_any_element()
    }
}

/// A round color swatch button; a ring marks the chosen one.
fn swatch(id: String, chosen: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .role(Role::Button)
        .size(px(SWATCH))
        .flex_shrink_0()
        .rounded_full()
        .cursor_pointer()
        .border_2()
        .border_color(if chosen {
            content(0.9)
        } else {
            gpui::transparent_black()
        })
        .font_weight(FontWeight::NORMAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input;
    use gpui::{
        Bounds, Entity, Render, TestAppContext, VisualTestContext, WindowBounds, WindowHandle,
        WindowOptions, point, size,
    };
    use monocode_core::today::statuses::{format_color, presets};
    use monocode_core::today::{Tag, tags as tag_rules};
    use std::sync::{Arc, Mutex};

    struct Host {
        today: Entity<Today>,
    }
    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.today.update(cx, |today, cx| {
                div()
                    .size_full()
                    .children(today.statuses_dialog_view(window, cx))
                    .children(today.tags_dialog_view(window, cx))
            })
        }
    }

    fn open(cx: &mut TestAppContext) -> (WindowHandle<Host>, VisualTestContext) {
        open_kind(cx, Kind::Statuses)
    }
    fn open_kind(cx: &mut TestAppContext, kind: Kind) -> (WindowHandle<Host>, VisualTestContext) {
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            crate::popover::bind(cx);
            crate::input::bind(cx);
        });
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1000.), px(900.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let store = Arc::new(Mutex::new(
                            monocode_core::store::Store::open(std::path::Path::new(":memory:"))
                                .unwrap(),
                        ));
                        Host {
                            today: cx.new(|cx| Today::new(store, cx)),
                        }
                    })
                },
            )
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .update(&mut visual, |host, _, cx| {
                host.today.update(cx, |today, cx| match kind {
                    Kind::Statuses => today.open_statuses(cx),
                    Kind::Tags => today.open_tags(cx),
                })
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        (window, visual)
    }
    fn tags(window: &WindowHandle<Host>, visual: &mut VisualTestContext) -> Vec<Tag> {
        let today = today(window, visual);
        today.read_with(visual, |today, _| today.tags.clone())
    }
    fn tag_input(
        window: &WindowHandle<Host>,
        visual: &mut VisualTestContext,
        id: &str,
    ) -> Entity<Input> {
        let today = today(window, visual);
        today.read_with(visual, |today, _| {
            today.tags_dialog.as_ref().unwrap().input(id).unwrap()
        })
    }
    /// Adds a tag straight through the store and returns it.
    fn add_tag(window: &WindowHandle<Host>, visual: &mut VisualTestContext, name: &str) -> Tag {
        let today = today(window, visual);
        let name = name.to_owned();
        today.update(visual, |today, cx| {
            let color = format_color(tag_rules::preset_for(today.tags.len()));
            today.add_tag(name, color, cx)
        });
        settle(visual);
        tags(window, visual).pop().unwrap()
    }
    fn tag(id: &str) -> Tag {
        Tag {
            id: id.into(),
            name: id.into(),
            color: format_color(tag_rules::PRESETS[0]),
        }
    }
    fn today(window: &WindowHandle<Host>, visual: &mut VisualTestContext) -> Entity<Today> {
        window
            .read_with(visual, |host, _| host.today.clone())
            .unwrap()
    }
    fn list(window: &WindowHandle<Host>, visual: &mut VisualTestContext) -> Vec<WorkflowStatus> {
        let today = today(window, visual);
        today.read_with(visual, |today, _| today.statuses.clone())
    }
    fn settle(visual: &mut VisualTestContext) {
        visual.run_until_parked();
        visual.refresh().unwrap();
    }
    fn click(visual: &mut VisualTestContext, selector: &str) {
        visual.refresh().unwrap();
        let at = bounds(visual, selector).unwrap_or_else(|| panic!("{selector} is drawn"));
        visual.simulate_click(at.center(), Default::default());
        settle(visual);
    }
    fn has(visual: &mut VisualTestContext, selector: &str) -> bool {
        visual.refresh().unwrap();
        bounds(visual, selector).is_some()
    }
    fn bounds(visual: &mut VisualTestContext, selector: &str) -> Option<Bounds<gpui::Pixels>> {
        // `debug_bounds` wants a 'static name; a test may leak a few bytes.
        visual.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
    }
    /// Types `text` into a name input the way a user does: focused, then Enter.
    fn rename(
        window: &WindowHandle<Host>,
        visual: &mut VisualTestContext,
        field: &Entity<Input>,
        text: &str,
    ) {
        window
            .update(visual, |_, window, cx| {
                window.focus(&field.read(cx).focus_handle(cx), cx)
            })
            .unwrap();
        field.update(visual, |field, cx| field.set_text(text.into(), cx));
        field.update(visual, |_, cx| cx.emit(input::Submitted));
        settle(visual);
    }
    fn add(
        window: &WindowHandle<Host>,
        visual: &mut VisualTestContext,
        name: &str,
        stage: Stage,
    ) -> WorkflowStatus {
        let today = today(window, visual);
        let name = name.to_owned();
        today.update(visual, |today, cx| {
            today.add_status(name, stage, format_color(presets(stage)[1]), cx)
        });
        settle(visual);
        list(window, visual)
            .into_iter()
            .rfind(|s| s.stage == stage)
            .unwrap()
    }

    fn status(id: &str, stage: Stage, builtin: bool) -> WorkflowStatus {
        WorkflowStatus {
            id: id.into(),
            name: id.into(),
            stage,
            color: format_color(presets(stage)[0]),
            builtin,
        }
    }

    #[test]
    fn new_statuses_get_the_first_free_default_name() {
        let mut list = vec![status("todo", Stage::NotStarted, true)];
        assert_eq!(default_name(&list), "New status");
        list.push(WorkflowStatus {
            name: "new STATUS".into(),
            ..status("a", Stage::Started, false)
        });
        assert_eq!(default_name(&list), "New status 2");
        list.push(WorkflowStatus {
            name: "New status 2".into(),
            ..status("b", Stage::Started, false)
        });
        assert_eq!(default_name(&list), "New status 3");
    }
    #[test]
    fn built_ins_only_drop_inside_their_own_stage() {
        let list = vec![
            status("todo", Stage::NotStarted, true),
            status("c1", Stage::NotStarted, false),
            status("in_progress", Stage::Started, true),
            status("c2", Stage::Started, false),
            status("done", Stage::Finished, true),
        ];
        assert_eq!(drop_target(&list, "todo", Stage::Started, None), None);
        assert_eq!(
            drop_target(&list, "todo", Stage::NotStarted, Some("c1")),
            Some((Stage::NotStarted, 1))
        );
        assert_eq!(
            drop_target(&list, "c2", Stage::NotStarted, None),
            Some((Stage::NotStarted, 2))
        );
        assert_eq!(
            drop_target(&list, "c2", Stage::Finished, Some("done")),
            Some((Stage::Finished, 0))
        );
        assert_eq!(drop_target(&list, "c2", Stage::Started, Some("c2")), None);
        assert_eq!(drop_target(&list, "missing", Stage::Started, None), None);
    }

    #[gpui::test]
    fn the_dialog_shows_three_sections_with_rows_in_order(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        add(&window, &mut visual, "In review", Stage::Started);
        for section in ["not_started", "started", "finished"] {
            assert!(
                has(&mut visual, &format!("statuses-section-{section}")),
                "{section}"
            );
        }
        let top = |visual: &mut VisualTestContext, id: &str| {
            bounds(visual, &format!("status-row-{id}"))
                .unwrap()
                .origin
                .y
        };
        let ids: Vec<String> = list(&window, &mut visual)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(ids.len(), 4);
        let ys: Vec<_> = ids.iter().map(|id| top(&mut visual, id)).collect();
        assert!(
            ys.windows(2).all(|w| w[0] < w[1]),
            "rows follow the list order"
        );
    }
    #[gpui::test]
    fn built_ins_say_built_in_and_cannot_be_deleted(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let custom = add(&window, &mut visual, "In review", Stage::Started);
        for id in ["todo", "in_progress", "done"] {
            assert!(has(&mut visual, &format!("status-builtin-{id}")), "{id}");
            assert!(!has(&mut visual, &format!("status-delete-{id}")), "{id}");
        }
        assert!(has(&mut visual, &format!("status-delete-{}", custom.id)));
        assert!(!has(&mut visual, &format!("status-builtin-{}", custom.id)));
    }
    #[gpui::test]
    fn add_puts_a_custom_status_at_the_end_of_that_stage(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        click(&mut visual, "status-add-started");
        click(&mut visual, "status-add-started");
        let started: Vec<_> = list(&window, &mut visual)
            .into_iter()
            .filter(|s| s.stage == Stage::Started)
            .collect();
        let names: Vec<_> = started.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["In progress", "New status", "New status 2"]);
        assert_eq!(started[1].color, format_color(presets(Stage::Started)[0]));
        assert!(!started[1].builtin);
    }
    #[gpui::test]
    fn delete_removes_a_custom_status(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let custom = add(&window, &mut visual, "In review", Stage::Started);
        click(&mut visual, &format!("status-delete-{}", custom.id));
        assert!(list(&window, &mut visual).iter().all(|s| s.id != custom.id));
        assert!(!has(&mut visual, &format!("status-row-{}", custom.id)));
    }
    #[gpui::test]
    fn a_swatch_recolors_the_status(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        click(&mut visual, "status-swatch-todo-1");
        let todo = list(&window, &mut visual).remove(0);
        assert_eq!(todo.color, format_color(presets(Stage::NotStarted)[1]));
    }
    #[gpui::test]
    fn the_gradient_swatch_opens_the_color_picker_and_a_pick_recolors(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        assert!(!has(&mut visual, "mono-custom-color-picker"));
        click(&mut visual, "status-custom-todo");
        assert!(has(&mut visual, "mono-custom-color-picker"));
        let today = today(&window, &mut visual);
        let picker = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .picker_view()
                .unwrap()
        });
        picker.update(&mut visual, |_, cx| cx.emit(Picked(0x654321)));
        settle(&mut visual);
        assert_eq!(list(&window, &mut visual)[0].color, "#654321");
        click(&mut visual, "status-custom-todo");
        assert!(!has(&mut visual, "mono-custom-color-picker"));
    }
    #[gpui::test]
    fn several_picks_in_a_row_store_the_last_color(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        click(&mut visual, "status-custom-todo");
        let today = today(&window, &mut visual);
        let picker = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .picker_view()
                .unwrap()
        });
        for color in [0x111111, 0x222222, 0x333333, 0x444444] {
            picker.update(&mut visual, |_, cx| cx.emit(Picked(color)));
        }
        // The dot follows at once, before any write finishes.
        assert_eq!(list(&window, &mut visual)[0].color, "#444444");
        settle(&mut visual);
        settle(&mut visual);
        let stored = today.read_with(&visual, |today, _| {
            monocode_core::today::statuses::parse_color(&today.statuses[0].color)
        });
        assert_eq!(stored, Some(0x444444));
        let db = today.read_with(&visual, |today, _| today.store.clone());
        let saved = db.lock().unwrap().today_statuses().unwrap();
        assert_eq!(saved[0].color, "#444444");
    }
    #[gpui::test]
    fn escape_commits_a_name_being_typed(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        window
            .update(&mut visual, |_, window, cx| {
                window.focus(&field.read(cx).focus_handle(cx), cx)
            })
            .unwrap();
        field.update(&mut visual, |field, cx| field.set_text("Queued".into(), cx));
        visual.simulate_keystrokes("escape");
        settle(&mut visual);
        settle(&mut visual);
        let db = today.read_with(&visual, |today, _| today.store.clone());
        assert_eq!(
            db.lock().unwrap().today_statuses().unwrap()[0].name,
            "Queued"
        );
    }
    #[gpui::test]
    fn a_taken_name_shows_its_error_inside_the_card(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        rename(&window, &mut visual, &field, "done");
        assert!(has(&mut visual, "statuses-error"));
    }
    #[gpui::test]
    fn a_rename_commits_on_enter(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        rename(&window, &mut visual, &field, "Backlog");
        assert_eq!(list(&window, &mut visual)[0].name, "Backlog");
    }
    #[gpui::test]
    fn an_empty_or_taken_name_is_put_back(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        for text in ["   ", "done"] {
            rename(&window, &mut visual, &field, text);
            assert_eq!(list(&window, &mut visual)[0].name, "To do");
            assert_eq!(
                field.read_with(&visual, |f, _| f.content.to_string()),
                "To do"
            );
        }
    }
    #[gpui::test]
    fn dropping_respects_the_built_in_rule(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let custom = add(&window, &mut visual, "In review", Stage::Started);
        let today = today(&window, &mut visual);
        today.update(&mut visual, |today, cx| {
            today.drop_status("todo", Stage::Started, None, cx)
        });
        settle(&mut visual);
        assert_eq!(list(&window, &mut visual)[0].stage, Stage::NotStarted);
        let id = custom.id.clone();
        today.update(&mut visual, |today, cx| {
            today.drop_status(&id, Stage::Finished, Some("done"), cx)
        });
        settle(&mut visual);
        let moved = list(&window, &mut visual)
            .into_iter()
            .find(|s| s.id == custom.id)
            .unwrap();
        assert_eq!(moved.stage, Stage::Finished);
        let order: Vec<_> = list(&window, &mut visual)
            .into_iter()
            .filter(|s| s.stage == Stage::Finished)
            .map(|s| s.id)
            .collect();
        assert_eq!(order, [custom.id, "done".to_owned()]);
    }
    #[gpui::test]
    fn every_name_input_has_the_same_width(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let custom = add(
            &window,
            &mut visual,
            "A much longer status name here",
            Stage::Finished,
        );
        let width = |visual: &mut VisualTestContext, id: &str| {
            visual.refresh().unwrap();
            bounds(visual, &format!("status-name-{id}"))
                .unwrap()
                .size
                .width
        };
        let first = width(&mut visual, "todo");
        assert!(first > px(100.));
        for id in ["in_progress", "done", custom.id.as_str()] {
            assert_eq!(width(&mut visual, id), first, "{id}");
        }
    }
    #[gpui::test]
    fn escape_and_done_close_the_dialog(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let closed = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = closed.clone();
        let _sub = visual.update(|_, cx| {
            cx.subscribe(&today, move |_, event: &crate::today::Event, _| {
                if matches!(event, crate::today::Event::Close) {
                    seen.set(true);
                }
            })
        });
        let open = |visual: &mut VisualTestContext| {
            today.read_with(visual, |today, _| today.statuses_dialog.is_some())
        };
        assert!(open(&mut visual));
        visual.simulate_keystrokes("escape");
        settle(&mut visual);
        assert!(!open(&mut visual));
        assert!(!closed.get(), "Escape in the dialog must not close Today");
        today.update(&mut visual, |today, cx| today.open_statuses(cx));
        settle(&mut visual);
        assert!(open(&mut visual));
        click(&mut visual, "statuses-done");
        assert!(!open(&mut visual));
    }
    #[gpui::test]
    fn a_name_being_edited_survives_a_refresh_and_new_rows_get_inputs(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        window
            .update(&mut visual, |_, window, cx| {
                window.focus(&field.read(cx).focus_handle(cx), cx)
            })
            .unwrap();
        field.update(&mut visual, |field, cx| field.set_text("Typing".into(), cx));
        today.update(&mut visual, |today, cx| {
            today.statuses[0].name = "Renamed elsewhere".into();
            cx.notify();
        });
        settle(&mut visual);
        assert_eq!(
            field.read_with(&visual, |f, _| f.content.to_string()),
            "Typing"
        );
        let custom = add(&window, &mut visual, "In review", Stage::Started);
        assert!(today.read_with(&visual, |t, _| {
            t.statuses_dialog
                .as_ref()
                .unwrap()
                .input(&custom.id)
                .is_some()
        }));
    }

    #[test]
    fn new_tags_get_the_first_free_default_name() {
        let mut list = vec![];
        assert_eq!(default_tag_name(&list), "New tag");
        list.push(Tag {
            name: " NEW tag ".into(),
            ..tag("a")
        });
        assert_eq!(default_tag_name(&list), "New tag 2");
        list.push(Tag {
            name: "New tag 2".into(),
            ..tag("b")
        });
        assert_eq!(default_tag_name(&list), "New tag 3");
    }
    #[test]
    fn a_dragged_tag_takes_the_place_of_the_row_it_lands_on() {
        let list = vec![tag("a"), tag("b"), tag("c")];
        assert_eq!(tag_drop_index(&list, "a", Some("c")), Some(2));
        assert_eq!(tag_drop_index(&list, "c", Some("a")), Some(0));
        assert_eq!(tag_drop_index(&list, "a", None), Some(3));
        assert_eq!(tag_drop_index(&list, "b", Some("b")), None);
        assert_eq!(tag_drop_index(&list, "missing", None), None);
        assert_eq!(tag_drop_index(&list, "a", Some("missing")), None);
    }
    #[test]
    fn the_delete_question_says_how_many_tasks_use_the_tag() {
        assert_eq!(delete_prompt(0), "Not used by any task. Delete it?");
        assert_eq!(
            delete_prompt(1),
            "Used by 1 task. Delete it and remove it from that task?"
        );
        assert_eq!(
            delete_prompt(3),
            "Used by 3 tasks. Delete it and remove it from them?"
        );
    }

    #[gpui::test]
    fn the_header_button_opens_the_tags_dialog(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let today = today(&window, &mut visual);
        today.read_with(&visual, |t, _| {
            assert!(t.tags_dialog.is_some());
            assert!(t.statuses_dialog.is_none());
        });
        assert!(has(&mut visual, "tags-section"));
        assert!(has(&mut visual, "tag-add"));
        // Opening Statuses closes Tags: never both.
        today.update(&mut visual, |t, cx| t.open_statuses(cx));
        today.read_with(&visual, |t, _| {
            assert!(t.statuses_dialog.is_some());
            assert!(t.tags_dialog.is_none());
        });
    }
    #[gpui::test]
    fn add_appends_a_tag_with_a_free_default_name(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        click(&mut visual, "tag-add");
        click(&mut visual, "tag-add");
        let list = tags(&window, &mut visual);
        let names: Vec<_> = list.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["New tag", "New tag 2"]);
        assert_eq!(list[0].color, format_color(tag_rules::preset_for(0)));
        assert_eq!(list[1].color, format_color(tag_rules::preset_for(1)));
        assert!(has(&mut visual, &format!("tag-row-{}", list[1].id)));
        assert!(!has(&mut visual, &format!("tag-builtin-{}", list[1].id)));
    }
    #[gpui::test]
    fn a_tag_rename_commits_on_enter(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let one = add_tag(&window, &mut visual, "Bug");
        let field = tag_input(&window, &mut visual, &one.id);
        rename(&window, &mut visual, &field, "Defect");
        assert_eq!(tags(&window, &mut visual)[0].name, "Defect");
    }
    #[gpui::test]
    fn an_empty_or_taken_tag_name_is_put_back(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        add_tag(&window, &mut visual, "Bug");
        let two = add_tag(&window, &mut visual, "Idea");
        let field = tag_input(&window, &mut visual, &two.id);
        rename(&window, &mut visual, &field, "   ");
        assert_eq!(tags(&window, &mut visual)[1].name, "Idea");
        assert_eq!(
            field.read_with(&visual, |f, _| f.content.to_string()),
            "Idea"
        );
        assert!(!has(&mut visual, "tags-error"));
        rename(&window, &mut visual, &field, " bUG ");
        assert_eq!(tags(&window, &mut visual)[1].name, "Idea");
        assert_eq!(
            field.read_with(&visual, |f, _| f.content.to_string()),
            "Idea"
        );
        assert!(has(&mut visual, "tags-error"));
        let message = today(&window, &mut visual).read_with(&visual, |t, _| t.action_error.clone());
        assert_eq!(
            message.as_deref(),
            Some("A tag with that name already exists")
        );
    }
    #[gpui::test]
    fn a_tag_swatch_recolors_the_tag(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let one = add_tag(&window, &mut visual, "Bug");
        click(&mut visual, &format!("tag-swatch-{}-3", one.id));
        assert_eq!(
            tags(&window, &mut visual)[0].color,
            format_color(tag_rules::PRESETS[3])
        );
    }
    #[gpui::test]
    fn the_tag_gradient_swatch_opens_the_picker_and_a_pick_recolors(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let one = add_tag(&window, &mut visual, "Bug");
        assert!(!has(&mut visual, "mono-custom-color-picker"));
        click(&mut visual, &format!("tag-custom-{}", one.id));
        assert!(has(&mut visual, "mono-custom-color-picker"));
        let today = today(&window, &mut visual);
        let picker = today.read_with(&visual, |t, _| {
            t.tags_dialog.as_ref().unwrap().picker_view().unwrap()
        });
        picker.update(&mut visual, |_, cx| cx.emit(Picked(0x654321)));
        settle(&mut visual);
        settle(&mut visual);
        assert_eq!(tags(&window, &mut visual)[0].color, "#654321");
        let db = today.read_with(&visual, |t, _| t.store.clone());
        assert_eq!(db.lock().unwrap().today_tags().unwrap()[0].color, "#654321");
    }
    #[gpui::test]
    fn dropping_a_tag_reorders_the_list(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let a = add_tag(&window, &mut visual, "A");
        let b = add_tag(&window, &mut visual, "B");
        let c = add_tag(&window, &mut visual, "C");
        let today = today(&window, &mut visual);
        let id = a.id.clone();
        today.update(&mut visual, |t, cx| t.drop_tag(&id, Some(&c.id), cx));
        settle(&mut visual);
        let order: Vec<_> = tags(&window, &mut visual)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(order, [b.id.clone(), c.id.clone(), a.id.clone()]);
        let id = a.id.clone();
        today.update(&mut visual, |t, cx| t.drop_tag(&id, Some(&b.id), cx));
        settle(&mut visual);
        let order: Vec<_> = tags(&window, &mut visual)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(order, [a.id, b.id, c.id]);
    }
    #[gpui::test]
    fn deleting_a_tag_asks_first_and_only_deletes_after_confirming(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let one = add_tag(&window, &mut visual, "Bug");
        let text = format!("tag-delete-text-{}", one.id);
        click(&mut visual, &format!("tag-delete-{}", one.id));
        settle(&mut visual);
        assert!(has(&mut visual, &text), "the question is shown");
        let shown = today(&window, &mut visual).read_with(&visual, |t, _| {
            t.tags_dialog.as_ref().unwrap().confirm.clone()
        });
        assert_eq!(shown, Some((one.id.clone(), 0)));
        assert_eq!(tags(&window, &mut visual).len(), 1, "nothing deleted yet");
        // Cancel keeps the tag.
        click(&mut visual, &format!("tag-delete-cancel-{}", one.id));
        assert!(!has(&mut visual, &text));
        assert_eq!(tags(&window, &mut visual).len(), 1);
        // Ask again and confirm.
        click(&mut visual, &format!("tag-delete-{}", one.id));
        click(&mut visual, &format!("tag-delete-confirm-{}", one.id));
        settle(&mut visual);
        assert!(tags(&window, &mut visual).is_empty());
        assert!(!has(&mut visual, &format!("tag-row-{}", one.id)));
    }
    #[gpui::test]
    fn the_delete_question_counts_the_tasks_using_the_tag(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let one = add_tag(&window, &mut visual, "Bug");
        let today = today(&window, &mut visual);
        let db = today.read_with(&visual, |t, _| t.store.clone());
        {
            let db = db.lock().unwrap();
            for title in ["First", "Second"] {
                db.today_add(
                    &monocode_core::today::NewTask {
                        title: title.into(),
                        tags: vec![one.id.clone()],
                        ..Default::default()
                    },
                    &monocode_core::today::Actor::User,
                    super::super::now_ms(),
                )
                .unwrap();
            }
        }
        click(&mut visual, &format!("tag-delete-{}", one.id));
        settle(&mut visual);
        let shown = today.read_with(&visual, |t, _| {
            t.tags_dialog.as_ref().unwrap().confirm.clone()
        });
        assert_eq!(shown, Some((one.id.clone(), 2)));
    }
    #[gpui::test]
    fn escape_and_done_close_the_tags_dialog(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let today = today(&window, &mut visual);
        let closed = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = closed.clone();
        let _sub = visual.update(|_, cx| {
            cx.subscribe(&today, move |_, event: &crate::today::Event, _| {
                if matches!(event, crate::today::Event::Close) {
                    seen.set(true);
                }
            })
        });
        let is_open = |visual: &mut VisualTestContext| {
            today.read_with(visual, |t, _| t.tags_dialog.is_some())
        };
        assert!(is_open(&mut visual));
        visual.simulate_keystrokes("escape");
        settle(&mut visual);
        assert!(!is_open(&mut visual));
        assert!(!closed.get(), "Escape in the dialog must not close Today");
        today.update(&mut visual, |t, cx| t.open_tags(cx));
        settle(&mut visual);
        assert!(is_open(&mut visual));
        click(&mut visual, "tags-done");
        assert!(!is_open(&mut visual));
    }

    #[gpui::test]
    fn a_pending_color_still_lands_when_the_dialog_closes(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let one = add_tag(&window, &mut visual, "Bug");
        click(&mut visual, &format!("tag-custom-{}", one.id));
        let today = today(&window, &mut visual);
        let picker = today.read_with(&visual, |t, _| {
            t.tags_dialog.as_ref().unwrap().picker_view().unwrap()
        });
        for color in [0x111111, 0x222222, 0x333333] {
            picker.update(&mut visual, |_, cx| cx.emit(Picked(color)));
        }
        // Close at once, while the first write is still in flight.
        today.update(&mut visual, |t, cx| t.close_tags(cx));
        // Reopen: the old write finishing must not disturb the new dialog.
        today.update(&mut visual, |t, cx| t.open_tags(cx));
        for _ in 0..4 {
            settle(&mut visual);
        }
        let db = today.read_with(&visual, |t, _| t.store.clone());
        assert_eq!(db.lock().unwrap().today_tags().unwrap()[0].color, "#333333");
        let queue = today.read_with(&visual, |t, _| {
            t.tags_dialog.as_ref().unwrap().colors.clone()
        });
        assert!(!queue.borrow().writing);
    }
    #[gpui::test]
    fn a_usage_count_for_a_closed_dialog_asks_nothing(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let one = add_tag(&window, &mut visual, "Bug");
        let today = today(&window, &mut visual);
        let id = one.id.clone();
        today.update(&mut visual, |t, cx| {
            t.ask_delete_tag(&id, cx);
            t.close_tags(cx);
            t.open_tags(cx);
        });
        settle(&mut visual);
        settle(&mut visual);
        today.read_with(&visual, |t, _| {
            assert!(t.tags_dialog.as_ref().unwrap().confirm.is_none())
        });
    }
    #[gpui::test]
    fn a_name_is_compared_after_it_is_cut_to_the_max_length(cx: &mut TestAppContext) {
        let (window, mut visual) = open_kind(cx, Kind::Tags);
        let long = "x".repeat(tag_rules::MAX_NAME);
        add_tag(&window, &mut visual, &long);
        let two = add_tag(&window, &mut visual, "Idea");
        let field = tag_input(&window, &mut visual, &two.id);
        rename(&window, &mut visual, &field, &format!("{long}yy"));
        assert_eq!(tags(&window, &mut visual)[1].name, "Idea");
        assert!(has(&mut visual, "tags-error"));
    }
}
