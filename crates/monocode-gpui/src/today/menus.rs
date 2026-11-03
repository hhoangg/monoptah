//! The page's one open dropdown: which trigger it hangs from, what each row
//! does, and the project and Mono marks the menus and rows share.
use super::Today;
use crate::{
    inbox::label_badge,
    popover::{Anchor, AnchorSlot, Menu, MenuAction, MenuEvent, MenuItem, MenuStyle, menu_popover},
};
use gpui::{AnyElement, Context, Div, Entity, SharedString, Subscription, Window, prelude::*};
use monocode_core::{inbox::Label, today::Priority};

/// What a picked menu row does.
#[derive(Clone)]
pub(super) enum Pick {
    Filters(super::Filters),
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
    Dismiss {
        id: String,
    },
    /// Start session on a task that had no project: set it, then start.
    StartWithProject {
        id: String,
        project: String,
    },
}
/// Which trigger a menu hangs from; also where its popover is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Slot {
    ProjectFilter,
    SourceFilter,
    MonoFilter,
    Priority,
    Project,
    /// The detail's status pill.
    Status,
    /// The detail's More menu (Snooze, Dismiss).
    More,
    /// A board column header's menu; only the open column's trigger writes the anchor.
    Column,
    /// Start session on a task with no project: pick one first.
    StartProject,
}
const SLOTS: usize = 9;
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
            Self::ProjectFilter | Self::SourceFilter | Self::MonoFilter => Host::Filters,
            Self::Column => Host::Board,
            _ => Host::Panel,
        }
    }
}
pub(super) struct OpenMenu {
    menu: Entity<Menu>,
    slot: Slot,
    picks: Vec<(SharedString, Pick)>,
    _events: Subscription,
}
/// The one open dropdown, and the anchors the triggers write while they lay out.
#[derive(Default)]
pub(crate) struct Menus {
    open: Option<OpenMenu>,
    anchors: [AnchorSlot; SLOTS],
}
impl Menus {
    pub(super) fn close(&mut self) {
        self.open = None;
    }
    #[cfg(test)]
    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
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
        let menu = cx.new(|cx| {
            Menu::new(label, items, window, cx)
                .style(MenuStyle::Filter)
                .width(240.)
        });
        let events = cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            let picked = match event {
                MenuEvent::Picked(id, _) => this.menus.open.as_ref().and_then(|open| {
                    open.picks
                        .iter()
                        .find(|(row, _)| row == id)
                        .map(|(_, pick)| pick.clone())
                }),
                MenuEvent::Dismissed => None,
            };
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
            _events: events,
        });
        cx.notify();
    }
    fn apply_pick(&mut self, pick: Pick, cx: &mut Context<Self>) {
        match pick {
            Pick::Filters(filters) => self.set_filters(filters, cx),
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
            Pick::Dismiss { id } => self.dismiss(id, cx),
            Pick::StartWithProject { id, project } => self.start_with_project(id, project, cx),
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
