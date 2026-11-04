//! The page's one open dropdown: which trigger it hangs from, what each row
//! does, and the project, Mono and tag chips and the Tags menu the menus and
//! rows share.
use super::Today;
use crate::runtime::locked;
use crate::{
    inbox::label_badge,
    input,
    popover::{
        Anchor, AnchorSlot, Menu, MenuAction, MenuEvent, MenuItem, MenuSearch, MenuStyle,
        menu_popover,
    },
};
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, SharedString, Subscription, Window, prelude::*,
};
use monocode_core::{
    inbox::Label,
    store::Store,
    time::now_ms,
    today::{Actor, Priority, Tag, tags},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// What a picked menu row does.
#[derive(Clone)]
pub(super) enum Pick {
    Filters(super::Filters),
    /// A multi-select filter row: applies, and the menu stays open.
    FilterToggle(super::Filters),
    Snooze {
        id: String,
        day: String,
    },
    Priority {
        id: String,
        priority: Priority,
    },
    Project {
        id: String,
        project: Option<String>,
    },
    /// Sets the task's workflow status.
    Status {
        id: String,
        status_id: String,
    },
    /// Opens the Statuses dialog.
    EditStatuses,
    /// Puts one tag on a task or takes it off; the menu stays open.
    TagToggle {
        id: String,
        tag_id: String,
        on: bool,
    },
    /// Puts a tag named `name` on the task, adding it first when no tag has
    /// the name; the menu stays open.
    TagCreate {
        id: String,
        name: String,
    },
    /// Opens the Tags dialog.
    EditTags,
    Dismiss {
        id: String,
    },
    /// Start session on a task that had no project: set it, then start.
    StartWithProject {
        id: String,
        project: String,
    },
    /// Start session menu: a new session (asks for a project first when the
    /// task has none).
    StartNew(Box<monocode_core::today::Task>),
    /// Start session menu: continue in an existing session of the task's project.
    ContinueIn {
        task: Box<monocode_core::today::Task>,
        session: i64,
    },
    /// Takes a session off its task.
    Unlink {
        session: i64,
    },
}
/// Which trigger a menu hangs from; also where its popover is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Slot {
    ProjectFilter,
    SourceFilter,
    MonoFilter,
    StatusFilter,
    TagFilter,
    Priority,
    Project,
    /// The detail's status pill.
    Status,
    /// The detail's Tags row.
    Tags,
    /// The detail's More menu (Snooze, Dismiss).
    More,
    /// A board column header's menu; only the open column's trigger writes the anchor.
    Column,
    /// Start session on a task with no project: pick one first.
    StartProject,
    /// The Start session split button's chevron.
    StartMenu,
    /// A row of the detail's SESSIONS section; only the open row's trigger writes the anchor.
    SessionMore,
}
const SLOTS: usize = 14;
/// Which part of the page draws a slot's popover.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Host {
    /// The filters row above the list or board.
    Filters,
    Board,
    Panel,
}
impl Slot {
    pub(super) fn host(self) -> Host {
        match self {
            Self::ProjectFilter
            | Self::SourceFilter
            | Self::MonoFilter
            | Self::StatusFilter
            | Self::TagFilter => Host::Filters,
            Self::Column => Host::Board,
            _ => Host::Panel,
        }
    }
}
pub(super) struct OpenMenu {
    menu: Entity<Menu>,
    slot: Slot,
    picks: Vec<(SharedString, Pick)>,
    /// Set on the Tags menu: what it edits and what it searches with.
    tags: Option<TagsMenu>,
    /// Set on the Start session menu: the task and the search field.
    start: Option<super::detail_sessions::ContinueMenu>,
    _events: Subscription,
}
/// The state behind the Tags menu: the task, the field, and the ids the task
/// holds as far as this menu knows (a toggle shows before the store answers).
struct TagsMenu {
    task: String,
    search: Entity<MenuSearch>,
    selected: Vec<String>,
    /// Writes still in flight for this menu.
    track: Rc<Track>,
    _changed: Subscription,
}
/// Tag writes of one open Tags menu that the store has not answered yet.
#[derive(Default)]
struct Track {
    pending: Cell<usize>,
    /// Names (lowercase) being created.
    creating: RefCell<Vec<String>>,
}
/// The one open dropdown, and the anchors the triggers write while they lay out.
#[derive(Default)]
pub(crate) struct Menus {
    open: Option<OpenMenu>,
    anchors: [AnchorSlot; SLOTS],
}
impl Pick {
    /// The menu stays open after this pick and is rebuilt in place.
    fn keeps_open(&self) -> bool {
        matches!(
            self,
            Self::FilterToggle(_) | Self::TagToggle { .. } | Self::TagCreate { .. }
        )
    }
}
impl Menus {
    pub(super) fn close(&mut self) {
        self.open = None;
    }
    #[cfg(test)]
    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }
    /// How many pickable rows the open menu holds.
    #[cfg(test)]
    pub(super) fn pick_count(&self) -> usize {
        self.open.as_ref().map_or(0, |open| open.picks.len())
    }
    /// The ids of the open menu's rows that do something.
    #[cfg(test)]
    pub(super) fn pick_ids(&self) -> Vec<String> {
        self.open
            .iter()
            .flat_map(|open| open.picks.iter().map(|(id, _)| id.to_string()))
            .collect()
    }
    /// The open menu's search field: the Tags menu's or the Start session menu's.
    #[cfg(test)]
    pub(super) fn search_input(&self, cx: &gpui::App) -> Option<Entity<input::Input>> {
        let open = self.open.as_ref()?;
        let search = match (&open.tags, &open.start) {
            (Some(menu), _) => &menu.search,
            (None, Some(menu)) => &menu.search,
            (None, None) => return None,
        };
        Some(search.read(cx).input.clone())
    }
}
/// Menu rows with what each one does.
#[derive(Default)]
pub(super) struct Rows {
    items: Vec<MenuItem>,
    picks: Vec<(SharedString, Pick)>,
}
impl Rows {
    pub(super) fn add(&mut self, action: MenuAction, pick: Pick) {
        self.picks.push((action.id.clone(), pick));
        self.items.push(action.into());
    }
    pub(super) fn separator(&mut self) {
        self.items.push(MenuItem::Separator);
    }
    pub(super) fn header(&mut self, text: impl Into<SharedString>) {
        self.items.push(MenuItem::Header(text.into()));
    }
    pub(super) fn note(&mut self, text: impl Into<SharedString>) {
        self.items.push(MenuItem::Note(text.into()));
    }
    /// A search field row, for a menu opened with a search field.
    pub(super) fn search(&mut self, search: &Entity<MenuSearch>, cx: &gpui::App) {
        self.items.push(MenuSearch::item(search, cx));
    }
}

impl Today {
    /// Opens `rows` under the trigger of `slot`, or closes it when it already is.
    /// Uses the app's `Menu`, so it takes focus, answers the arrow keys, Enter
    /// and Escape, and scrolls past the popover's height cap.
    pub(super) fn open_menu(
        &mut self,
        slot: Slot,
        label: &'static str,
        rows: Rows,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu_with(slot, label, rows, None, None, window, cx);
    }
    /// Opens the Start session menu, which has a search field.
    pub(super) fn open_menu_for(
        &mut self,
        slot: Slot,
        label: &'static str,
        rows: Rows,
        start: super::detail_sessions::ContinueMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu_with(slot, label, rows, None, Some(start), window, cx);
    }
    #[allow(clippy::too_many_arguments)]
    fn open_menu_with(
        &mut self,
        slot: Slot,
        label: &'static str,
        rows: Rows,
        tags: Option<TagsMenu>,
        start: Option<super::detail_sessions::ContinueMenu>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .menus
            .open
            .as_ref()
            .is_some_and(|open| open.slot == slot)
        {
            self.menus.open = None;
            cx.notify();
            return;
        }
        let items = rows.items;
        let searchable = tags.is_some() || start.is_some();
        let menu = cx.new(|cx| {
            let menu = Menu::new(label, items, window, cx)
                .style(MenuStyle::Filter)
                .width(240.);
            if searchable {
                menu.search_navigation()
            } else {
                menu
            }
        });
        let events = cx.subscribe_in(&menu, window, |this, _, event: &MenuEvent, window, cx| {
            let picked = match event {
                MenuEvent::Picked(id, _) => this.menus.open.as_ref().and_then(|open| {
                    open.picks
                        .iter()
                        .find(|(row, _)| row == id)
                        .map(|(_, pick)| pick.clone())
                }),
                MenuEvent::Dismissed => None,
            };
            // A new session may have to ask for a project, which needs the window.
            if let Some(Pick::StartNew(task)) = picked.clone() {
                this.menus.open = None;
                this.start_session(*task, window, cx);
                cx.notify();
                return;
            }
            if let Some(pick) = picked.as_ref().filter(|p| p.keeps_open()).cloned() {
                this.apply_pick(pick, cx);
                this.rebuild_open_menu(cx);
                // Typing goes on in the Tags menu's search field.
                if let Some(menu) = this.menus.open.as_ref().and_then(|open| open.tags.as_ref()) {
                    let focus = menu.search.read(cx).input.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                }
                cx.notify();
                return;
            }
            this.menus.open = None;
            if let Some(pick) = picked {
                this.apply_pick(pick, cx);
            }
            cx.notify();
        });
        self.menus.open = Some(OpenMenu {
            menu,
            slot,
            picks: rows.picks,
            tags,
            start,
            _events: events,
        });
        cx.notify();
    }
    /// Rebuilds the open menu's rows from the current filters or tags, in place.
    pub(super) fn rebuild_open_menu(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.menus.open.as_ref() else {
            return;
        };
        let rows = match (&open.tags, &open.start) {
            (Some(menu), _) => {
                self.tag_rows(&menu.task, &menu.selected, &menu.search, &menu.track, cx)
            }
            (None, Some(menu)) => self.start_rows(&menu.task, &menu.search, cx),
            (None, None) => self.filter_rows(open.slot),
        };
        if let Some(open) = self.menus.open.as_mut() {
            let filter_menu = open.tags.is_none() && open.start.is_none();
            open.menu.update(cx, |menu, cx| {
                menu.replace_items(rows.items, cx);
                // The filter menus have no search field: keep a row highlighted for the keys.
                if filter_menu {
                    menu.ensure_active(cx);
                }
            });
            open.picks = rows.picks;
        }
    }
    /// A load landed: an open filter menu follows the filters, statuses and tags.
    pub(super) fn refresh_filter_menu(&mut self, cx: &mut Context<Self>) {
        let is_filter_menu = self
            .menus
            .open
            .as_ref()
            .is_some_and(|open| open.tags.is_none() && open.slot.host() == Host::Filters);
        if is_filter_menu {
            self.rebuild_open_menu(cx);
        }
    }
    /// The store answered: the open Tags menu follows what the task now holds
    /// and the tags that now exist.
    pub(super) fn refresh_tags_menu(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.menus.open.as_ref() else {
            return;
        };
        let Some(menu) = &open.tags else {
            return;
        };
        let held = self
            .view
            .as_ref()
            .and_then(|view| view.rows.iter().find(|row| row.task.id == menu.task))
            .map(|row| row.tags.iter().map(|tag| tag.id.clone()).collect());
        let Some(held) = held else {
            self.menus.open = None;
            return;
        };
        if let Some(menu) = self.menus.open.as_mut().and_then(|open| open.tags.as_mut()) {
            // A load that began before our own writes landed would undo them.
            if menu.track.pending.get() == 0 {
                menu.selected = held;
            }
        }
        self.rebuild_open_menu(cx);
    }
    /// Opens the Tags menu of a task under the detail's Tags row, or closes it.
    pub(super) fn open_tags_menu(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected: Vec<String> = self
            .view
            .as_ref()
            .and_then(|view| view.rows.iter().find(|row| row.task.id == id))
            .map(|row| row.tags.iter().map(|tag| tag.id.clone()).collect())
            .unwrap_or_default();
        let search = MenuSearch::new("Search or create tags", cx);
        let input = search.read(cx).input.clone();
        let changed = cx.subscribe(&input, |this, _, _: &input::Changed, cx| {
            this.menu_search_changed(cx)
        });
        let track = Rc::new(Track::default());
        let rows = self.tag_rows(&id, &selected, &search, &track, cx);
        let menu = TagsMenu {
            task: id,
            search,
            selected,
            track,
            _changed: changed,
        };
        self.open_menu_with(Slot::Tags, "Tags", rows, Some(menu), None, window, cx);
    }
    /// The typed text of an open menu's search field picks the rows: rebuild
    /// them, and Enter acts on the first one.
    pub(super) fn menu_search_changed(&mut self, cx: &mut Context<Self>) {
        self.rebuild_open_menu(cx);
        if let Some(open) = self.menus.open.as_ref() {
            open.menu.update(cx, |menu, cx| menu.highlight_first(cx));
        }
    }
    /// The Tags menu's rows: the search field, a row per tag that matches it,
    /// a Create row when it names no tag, then Edit tags.
    fn tag_rows(
        &self,
        task: &str,
        selected: &[String],
        search: &Entity<MenuSearch>,
        track: &Track,
        cx: &gpui::App,
    ) -> Rows {
        let typed = search.read(cx).query(cx);
        let query = typed.trim();
        let full = selected.len() >= tags::MAX_PER_TASK;
        let mut rows = Rows::default();
        rows.items.push(MenuSearch::item(search, cx));
        let mut shown = 0;
        for tag in &self.tags {
            if !tag.name.to_lowercase().contains(&query.to_lowercase()) {
                continue;
            }
            shown += 1;
            let on = selected.contains(&tag.id);
            rows.add(
                MenuAction::new(format!("tag-{}", tag.id), &tag.name)
                    .icon("circle-dot")
                    .icon_color(gpui::rgb(tag.rgb()).into())
                    .checked(on)
                    .disabled(full && !on)
                    .keep_open(),
                Pick::TagToggle {
                    id: task.to_string(),
                    tag_id: tag.id.clone(),
                    on: !on,
                },
            );
        }
        let exists = self
            .tags
            .iter()
            .any(|tag| tags::same_name(&tag.name, query));
        let creating = track.creating.borrow().contains(&query.to_lowercase());
        if !query.is_empty() && !exists && !creating {
            let long = query.chars().count() > tags::MAX_NAME;
            rows.add(
                MenuAction::new("tag-create", format!("Create tag \"{query}\""))
                    .icon("plus")
                    .disabled(full || long)
                    .keep_open(),
                Pick::TagCreate {
                    id: task.to_string(),
                    name: query.to_string(),
                },
            );
        } else if shown == 0 && self.tags.is_empty() {
            rows.note("No tags yet. Type a name to create one.");
        }
        if full {
            rows.note(format!("A task has at most {} tags", tags::MAX_PER_TASK));
        }
        rows.separator();
        rows.add(
            MenuAction::new("tag-edit", "Edit tags").icon("sliders"),
            Pick::EditTags,
        );
        rows
    }
    /// Runs a tag write off the UI thread like `act`, and counts it in `track`
    /// until the store has answered, so the menu does not trust a load before then.
    fn tag_write(
        &mut self,
        work: impl FnOnce(&Store, u64) -> Result<(), String> + Send + 'static,
        track: Option<(Rc<Track>, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        if let Some((track, creating)) = &track {
            track.pending.set(track.pending.get() + 1);
            if let Some(name) = creating {
                track.creating.borrow_mut().push(name.clone());
            }
        }
        let store = self.store.clone();
        let task = cx
            .background_executor()
            .spawn(async move { work(&*locked(&store)?, now_ms()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            if let Some((track, creating)) = &track {
                track.pending.set(track.pending.get().saturating_sub(1));
                if let Some(name) = creating {
                    let mut names = track.creating.borrow_mut();
                    if let Some(at) = names.iter().position(|n| n == name) {
                        names.remove(at);
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.action_error = result.err();
                this.refresh(cx);
            });
        })
        .detach();
    }
    /// Puts the tag named `name` on the task: an existing tag of that name, or
    /// one added now with the next preset color.
    fn create_task_tag(
        &mut self,
        id: String,
        name: String,
        track: Option<Rc<Track>>,
        cx: &mut Context<Self>,
    ) {
        let color = tags::format_color(tags::preset_for(self.tags.len()));
        let key = name.to_lowercase();
        self.tag_write(
            move |store, now| {
                // A Mono or another view may have made it meanwhile.
                let existing = store
                    .today_tags()?
                    .into_iter()
                    .find(|tag| tags::same_name(&tag.name, &name));
                let tag = match existing {
                    Some(tag) => tag,
                    None => store.today_add_tag(&name, &color)?,
                };
                store
                    .today_toggle_tag(&id, &tag.id, true, &Actor::User, now)
                    .map(|_| ())
            },
            track.map(|track| (track, Some(key))),
            cx,
        );
    }
    fn apply_pick(&mut self, pick: Pick, cx: &mut Context<Self>) {
        match pick {
            Pick::Filters(filters) | Pick::FilterToggle(filters) => self.set_filters(filters, cx),
            Pick::Snooze { id, day } => self.snooze(id, day, cx),
            Pick::Priority { id, priority } => self.patch(
                id,
                monocode_core::today::TaskPatch {
                    priority: Some(priority),
                    ..Default::default()
                },
                cx,
            ),
            Pick::Project { id, project } => self.patch(
                id,
                monocode_core::today::TaskPatch {
                    project: Some(project),
                    ..Default::default()
                },
                cx,
            ),
            Pick::Status { id, status_id } => self.set_status(id, status_id, cx),
            Pick::EditStatuses => self.open_statuses(cx),
            Pick::TagToggle { id, tag_id, on } => {
                let mut track = None;
                if let Some(menu) = self.menus.open.as_mut().and_then(|open| open.tags.as_mut()) {
                    menu.selected.retain(|held| *held != tag_id);
                    if on {
                        menu.selected.push(tag_id.clone());
                    }
                    track = Some(menu.track.clone());
                }
                self.tag_write(
                    move |store, now| {
                        store
                            .today_toggle_tag(&id, &tag_id, on, &Actor::User, now)
                            .map(|_| ())
                    },
                    track.map(|track| (track, None)),
                    cx,
                );
            }
            Pick::TagCreate { id, name } => {
                // The typed name has become a tag: clear the field.
                if let Some(menu) = self.menus.open.as_ref().and_then(|open| open.tags.as_ref()) {
                    let input = menu.search.read(cx).input.clone();
                    input.update(cx, |input, cx| input.set_text(String::new(), cx));
                }
                let track = self
                    .menus
                    .open
                    .as_ref()
                    .and_then(|open| open.tags.as_ref())
                    .map(|menu| menu.track.clone());
                self.create_task_tag(id, name, track, cx);
            }
            Pick::EditTags => self.open_tags(cx),
            Pick::Dismiss { id } => self.dismiss(id, cx),
            Pick::StartWithProject { id, project } => self.start_with_project(id, project, cx),
            // Needs the window: handled where the menu reports the pick.
            Pick::StartNew(_) => {}
            Pick::ContinueIn { task, session } => self.continue_in(*task, session, cx),
            Pick::Unlink { session } => self.unlink_session(session, cx),
        }
    }
    /// The open menu's popover, drawn by the part of the page that owns its trigger.
    pub(super) fn menu_view(&self, host: Host, cx: &gpui::App) -> Option<AnyElement> {
        let open = self
            .menus
            .open
            .as_ref()
            .filter(|open| open.slot.host() == host)?;
        let anchor = self.menus.anchors[open.slot as usize].clone();
        Some(menu_popover(&open.menu, Anchor::Slot(anchor), cx).into_any_element())
    }
    pub(super) fn menus_open(&self, slot: Slot) -> bool {
        self.menus
            .open
            .as_ref()
            .is_some_and(|open| open.slot == slot)
    }
    pub(super) fn anchor(&self, slot: Slot) -> AnchorSlot {
        self.menus.anchors[slot as usize].clone()
    }
    /// A project's folder row in a menu, tinted with the project's own color.
    pub(super) fn project_row(
        &self,
        id: String,
        label: String,
        path: &str,
        checked: bool,
    ) -> MenuAction {
        let color = self
            .appearances
            .get(path)
            .cloned()
            .unwrap_or_default()
            .color(path);
        MenuAction::new(id, label)
            .icon("folder")
            .icon_color(gpui::rgb(color).into())
            .checked(checked)
    }
    /// The project badge of a row or a source: the project's own color, or a stable one.
    pub(super) fn project_badge(&self, path: &str, compact: bool) -> Div {
        let color = self
            .appearances
            .get(path)
            .cloned()
            .unwrap_or_default()
            .color(path);
        label_badge(
            &Label {
                name: self.project_label(path),
                color: format!("#{color:06x}"),
            },
            compact,
        )
    }
    /// A row's tags as colored badges, in the order given (definition order).
    /// `scope` names the debug selectors, `<scope>-<task>-<tag>`. With a `limit`,
    /// the tags past it fold into one neutral "+N" badge.
    pub(super) fn tag_chips(
        &self,
        scope: &'static str,
        task: &str,
        tags: &[Tag],
        compact: bool,
        limit: Option<usize>,
    ) -> Vec<Div> {
        let shown = limit.map_or(tags.len(), |limit| limit.min(tags.len()));
        let mut chips: Vec<Div> = tags[..shown]
            .iter()
            .map(|tag| {
                let selector = format!("{scope}-{task}-{}", tag.id);
                label_badge(
                    &Label {
                        name: tag.name.clone(),
                        color: tag.color.clone(),
                    },
                    compact,
                )
                .debug_selector(move || selector.clone())
                .flex_shrink_0()
            })
            .collect();
        if shown < tags.len() {
            let selector = format!("{scope}-{task}-more");
            chips.push(
                label_badge(
                    &Label {
                        name: format!("+{}", tags.len() - shown),
                        color: String::new(),
                    },
                    compact,
                )
                .debug_selector(move || selector.clone())
                .flex_shrink_0(),
            );
        }
        chips
    }
    /// Projects a task can belong to: the Monos' projects, the ones with an
    /// appearance and the ones on screen, by display name.
    pub(super) fn project_options(&self) -> Vec<(String, String)> {
        let mut paths = std::collections::BTreeSet::new();
        paths.extend(self.roster.iter().flat_map(|mono| mono.projects.clone()));
        paths.extend(self.appearances.keys().cloned());
        if let Some(view) = &self.view {
            paths.extend(view.rows.iter().filter_map(|row| row.task.project.clone()));
        }
        let mut options = paths
            .into_iter()
            .map(|path| (self.project_label(&path), path))
            .collect::<Vec<_>>();
        options.sort_by_key(|(label, _)| label.to_lowercase());
        options
    }
    pub(super) fn mono_mark(&self, id: &str, side: f32) -> Option<AnyElement> {
        let mono = self.mono(id)?;
        Some(crate::monos::avatar::avatar(
            &mono.mascot,
            mono.color,
            "idle",
            side,
            true,
        ))
    }
}
