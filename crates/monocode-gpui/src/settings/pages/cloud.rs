//! Settings > Cloud: the one global kit (design 3.2.2). Skills, MCP servers and plugins that
//! Monoptah copies from this computer to new cloud workspaces. Everything compatible is
//! checked by default, secrets included and OAuth-style servers carried config-only;
//! local-only items cannot be checked.
//!
//! Only item ids are stored and shown. A server's secret values never reach this page.
use crate::{
    settings::{
        SettingsView,
        widgets::{checkbox, note, red, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, Role, SharedString, Task, div, prelude::*, px};
use monocode_core::cloud_sessions::{
    kit::{KitInventory, KitSource, MAX_KIT_COMPRESSED_BYTES, Workspace},
    kit_prefs::{self, Badge, Enable, KitRow, KitSummary, KitTab, NOTICE},
};
use std::{path::Path, sync::Arc};

pub(crate) enum Load {
    /// Nothing asked yet, or no project to ask about.
    Idle,
    Loading,
    Ready(Arc<KitInventory>),
    Failed(String),
}

pub(in crate::settings) struct CloudState {
    tab: KitTab,
    pub(in crate::settings) load: Load,
    pub(in crate::settings) backends: super::cloud_backends::BackendsState,
    pub(in crate::settings) docker: super::cloud_docker::DockerState,
    /// Counts answers so a slow scan of the previous project is dropped.
    generation: u64,
    task: Option<Task<()>>,
    /// The one-time notice is open and waiting for the user.
    notice_open: bool,
}

impl Default for CloudState {
    fn default() -> Self {
        Self {
            tab: KitTab::Skills,
            load: Load::Idle,
            backends: Default::default(),
            docker: Default::default(),
            generation: 0,
            task: None,
            notice_open: false,
        }
    }
}

fn tab_key(tab: KitTab) -> &'static str {
    tab.id()
}

impl SettingsView {
    /// Reads the inventory of the selected project on a worker thread.
    pub(in crate::settings) fn refresh_cloud(&mut self, cx: &mut Context<Self>) {
        self.refresh_backends(cx);
        self.refresh_docker(cx);
        let project = self.selected_project.clone();
        self.cloud.generation += 1;
        let generation = self.cloud.generation;
        if project.is_empty() || project == "~" {
            self.cloud.load = Load::Idle;
            self.cloud.task = None;
            cx.notify();
            return;
        }
        self.cloud.load = Load::Loading;
        let scan = crate::cloud_kit_ui::scanner(cx);
        // Settings shows the codespace view; a start re-checks for its own workspace.
        let work = cx
            .background_executor()
            .spawn(async move { scan(Path::new(&project), Workspace::Codespace) });
        self.cloud.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.cloud.generation != generation {
                    return;
                }
                this.cloud.load = match result {
                    Ok(inventory) => Load::Ready(Arc::new(inventory)),
                    Err(error) => Load::Failed(error),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn set_kit_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if !on {
            self.cloud.notice_open = false;
            self.update(cx, |p| p.cloud_kit.disable());
            return;
        }
        let mut next = self.prefs.cloud_kit.clone();
        match next.request_enable() {
            Enable::Enabled => self.update(cx, |p| p.cloud_kit = next),
            Enable::NeedsNotice => {
                self.cloud.notice_open = true;
                cx.notify();
            }
        }
    }

    fn accept_kit_notice(&mut self, cx: &mut Context<Self>) {
        self.cloud.notice_open = false;
        self.update(cx, |p| p.cloud_kit.acknowledge_notice());
    }

    fn toggle_kit_item(&mut self, id: &str, checked: bool, cx: &mut Context<Self>) {
        let Load::Ready(inventory) = &self.cloud.load else {
            return;
        };
        let inventory = inventory.clone();
        self.update(cx, |p| p.cloud_kit.set_selected(&inventory, id, checked));
    }

    pub(super) fn cloud_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let kit = self.prefs.cloud_kit.clone();
        let mut page = vec![self.backends_group(cx), self.docker_group(cx)];
        page.push(self.group(
            Some("cloud-kit"),
            false,
            "Claude setup",
            Some(
                "Copy your skills, MCP servers, plugins and instructions to new cloud workspaces, so Claude works there the way it does here."
                    .into(),
            ),
            None,
            vec![self.row(
                Some("cloud-kit-enabled"),
                true,
                "Copy my Claude setup",
                Some("Applies to every new codespace and Docker sandbox. Each start can still turn it off.".into()),
                vec![self.toggle(
                    "cloud-kit-enabled",
                    "Copy my Claude setup",
                    kit.enabled,
                    false,
                    cx,
                    |this, on, cx| this.set_kit_enabled(on, cx),
                )],
            )],
        ));
        if self.cloud.notice_open {
            page.push(self.kit_notice(cx));
        }
        page.push(self.kit_instructions(&kit, cx));
        page.push(self.kit_items(&kit, cx));
        page
    }

    /// The "Your instructions (CLAUDE.md)" switch with what it would copy: the source, the
    /// file count and size, and the imports that stay behind.
    fn kit_instructions(
        &mut self,
        kit: &kit_prefs::CloudKitPrefs,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let found = match &self.cloud.load {
            Load::Ready(inventory) => inventory.instructions.clone(),
            _ => None,
        };
        let (description, disabled) = match (&self.cloud.load, &found) {
            (Load::Idle, _) => (
                "Open a project to see what can be copied.".to_owned(),
                false,
            ),
            (Load::Loading, _) => ("Reading your Claude setup...".to_owned(), false),
            (Load::Failed(error), _) => (error.clone(), false),
            (Load::Ready(_), None) => (
                "No CLAUDE.md found on this computer, so there is nothing to copy.".to_owned(),
                true,
            ),
            (Load::Ready(_), Some(found)) => match found.exclusion {
                Some(exclusion) => (
                    format!(
                        "Cannot be copied: {}.",
                        kit_prefs::exclusion_reason(exclusion)
                    ),
                    true,
                ),
                None if found.files.is_empty() => (
                    "Your CLAUDE.md cannot be copied: see below.".to_owned(),
                    true,
                ),
                None => (
                    format!(
                        "From {}: {} - {}. Copied with your CLAUDE.md imports and rules.",
                        match found.source {
                            KitSource::Account => "this account's Claude folder",
                            _ => "~/.claude",
                        },
                        match found.files.len() {
                            1 => "1 file".to_owned(),
                            n => format!("{n} files"),
                        },
                        kit_prefs::size_label(found.size),
                    ),
                    false,
                ),
            },
        };
        let mut children = vec![self.row(
            Some("cloud-kit-instructions"),
            true,
            "Your instructions (CLAUDE.md)",
            Some(description.into()),
            vec![self.toggle(
                "cloud-kit-instructions",
                "Your instructions (CLAUDE.md)",
                kit.instructions && !disabled,
                disabled,
                cx,
                |this, on, cx| this.update(cx, |p| p.cloud_kit.instructions = on),
            )],
        )];
        if let Some(found) = &found {
            let lines = found
                .not_carried
                .iter()
                .map(|(path, reason)| (format!("Not copied: {path} ({reason})"), true))
                .chain(
                    found
                        .notes
                        .iter()
                        .map(|(path, note)| (format!("Copied, but: {path} ({note})"), false)),
                );
            for (ix, (text, bad)) in lines.enumerate() {
                children.push(
                    div()
                        .id(("cloud-kit-instructions-warning", ix))
                        .debug_selector(move || format!("cloud-kit-instructions-warning-{ix}"))
                        .px_4()
                        .py(px(8.))
                        .border_t_1()
                        .border_color(content(0.05))
                        .text_size(px(12.))
                        .text_color(if bad { red(0.9) } else { content(0.55) })
                        .child(text)
                        .into_any_element(),
                );
            }
        }
        self.group(
            Some("cloud-kit-instructions-group"),
            false,
            "Instructions",
            Some("Your personal CLAUDE.md, the Markdown files it imports and your rules, so Claude follows them in the cloud too.".into()),
            None,
            children,
        )
    }

    fn kit_notice(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("cloud-kit-notice")
            .debug_selector(|| "cloud-kit-notice".into())
            .role(Role::AlertDialog)
            .aria_label("Before your setup is copied")
            .mt_4()
            .rounded_xl()
            .border_1()
            .border_color(content(0.2))
            .bg(content(0.05))
            .p_4()
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(content(0.9))
                    .child(NOTICE),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .gap_2()
                    .child(
                        secondary_button("cloud-kit-notice-accept", "Copy my setup", false, false)
                            .debug_selector(|| "cloud-kit-notice-accept".into())
                            .child("Copy my setup")
                            .on_click(cx.listener(|this, _, _, cx| this.accept_kit_notice(cx))),
                    )
                    .child(
                        secondary_button("cloud-kit-notice-cancel", "Cancel", false, false)
                            .debug_selector(|| "cloud-kit-notice-cancel".into())
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cloud.notice_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn kit_items(&mut self, kit: &kit_prefs::CloudKitPrefs, cx: &mut Context<Self>) -> AnyElement {
        let project = self.project_picker(cx);
        let inventory = match &self.cloud.load {
            Load::Ready(inventory) => Some(inventory.clone()),
            _ => None,
        };
        let summary = inventory
            .as_ref()
            .map(|inventory| KitSummary::of(inventory, kit));
        let tab = self.cloud.tab;
        let options: Vec<(&'static str, String)> = KitTab::ALL
            .into_iter()
            .map(|t| {
                let count = summary.map(|s| match t {
                    KitTab::Skills => s.skills,
                    KitTab::McpServers => s.mcp_servers,
                    KitTab::Plugins => s.plugins,
                });
                (
                    tab_key(t),
                    match count {
                        Some(count) => format!("{} {count}", t.label()),
                        None => t.label().to_owned(),
                    },
                )
            })
            .collect();
        let options: Vec<(&'static str, &str)> = options
            .iter()
            .map(|(id, label)| (*id, label.as_str()))
            .collect();
        let tabs = self.segmented(
            "cloud-kit-tab",
            "Kind of item",
            tab_key(tab),
            &options,
            cx,
            |this, value, cx| {
                if let Some(tab) = KitTab::ALL.into_iter().find(|t| t.id() == value) {
                    this.cloud.tab = tab;
                    cx.notify();
                }
            },
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        match (&self.cloud.load, &inventory) {
            (Load::Idle, _) => rows.push(note("Open a project to see what can be copied.")),
            (Load::Loading, _) => rows.push(note("Reading your Claude setup...")),
            (Load::Failed(error), _) => rows.push(
                div()
                    .id("settings-page-error")
                    .role(Role::Alert)
                    .px_4()
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xf87171))
                    .child(error.clone())
                    .into_any_element(),
            ),
            (Load::Ready(_), Some(inventory)) => {
                let list = kit_prefs::rows(inventory, kit, tab);
                if list.is_empty() {
                    rows.push(note(match tab {
                        KitTab::Skills => "No skills to copy.",
                        KitTab::McpServers => "No MCP servers to copy.",
                        KitTab::Plugins => "No enabled plugins to copy.",
                    }));
                }
                for (ix, row) in list.into_iter().enumerate() {
                    rows.push(self.kit_row(ix, row, cx));
                }
                if inventory.scan_timed_out {
                    rows.push(note(
                        "Reading some items took too long. They are listed but cannot be copied. Press Refresh to try again.",
                    ));
                }
                if let Some(text) = kit_prefs::key_unavailable_note(inventory) {
                    rows.push(note(text));
                }
                let (bytes, _) = kit_prefs::selected_compressed(inventory, kit);
                if bytes > MAX_KIT_COMPRESSED_BYTES {
                    rows.push(note(format!(
                        "The checked items are about {} compressed, more than the {} a workspace takes. Uncheck some items.",
                        kit_prefs::size_label(bytes),
                        kit_prefs::size_label(MAX_KIT_COMPRESSED_BYTES),
                    )));
                }
            }
            (Load::Ready(_), None) => {}
        }
        let reset = secondary_button(
            "cloud-kit-reset",
            "Use the defaults",
            false,
            kit.selection.is_none(),
        )
        .debug_selector(|| "cloud-kit-reset".into())
        .child("Use the defaults")
        .when(kit.selection.is_some(), |d| {
            d.on_click(cx.listener(|this, _, _, cx| {
                this.update(cx, |p| p.cloud_kit.reset_selection());
            }))
        });
        self.group(
            Some("cloud-kit-items"),
            false,
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_3()
                .child("What to copy")
                .child(project),
            Some(
                "One selection for every cloud workspace. Items that only work on this computer cannot be checked. Docker servers do not work in Docker sandboxes. Items of a single project follow that project."
                    .into(),
            ),
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(tabs)
                    .child(
                        secondary_button("refresh-cloud", "Refresh", false, false)
                            .debug_selector(|| "refresh-cloud".into())
                            .child(icon("refresh-cw", 14.))
                            .child("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(reset)
                    .into_any_element(),
            ),
            rows,
        )
    }

    fn kit_row(&self, ix: usize, row: KitRow, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id.clone();
        // A project item follows its own project: shown checked, not editable here.
        let selectable = row.selectable && !row.locked;
        let checked = row.checked;
        let selector = format!("cloud-kit-row-{}", row.id);
        let box_selector = format!("cloud-kit-check-{}", row.id);
        div()
            .id(("cloud-kit-row", ix))
            .debug_selector(move || selector.clone())
            .role(Role::ListItem)
            .aria_label(row.name.clone())
            .px_4()
            .py(px(10.))
            .when(ix > 0, |d| d.border_t_1().border_color(content(0.05)))
            .flex()
            .items_start()
            .gap_3()
            .when(selectable, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_kit_item(&id, !checked, cx);
                    }))
            })
            .child(checkbox(
                SharedString::from(box_selector),
                row.name.clone(),
                checked,
                !selectable,
            ))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(content(if selectable { 1. } else { 0.55 }))
                                    .child(row.name.clone()),
                            )
                            .children(row.badges.iter().map(|badge| kit_badge(*badge))),
                    )
                    .child(
                        div()
                            .mt(px(2.))
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child(match row.size {
                                Some(size) => {
                                    format!("{} - {}", row.detail, kit_prefs::size_label(size))
                                }
                                None => row.detail.clone(),
                            }),
                    )
                    .when_some(
                        row.locked
                            .then(|| "Follows its project automatically".to_owned())
                            .filter(|_| row.selectable)
                            .or_else(|| row.reason.clone()),
                        |d, reason| {
                            d.child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(12.))
                                    .text_color(content(0.55))
                                    .child(reason),
                            )
                        },
                    ),
            )
            .into_any_element()
    }
}

fn kit_badge(badge: Badge) -> AnyElement {
    let bad = matches!(badge, Badge::LocalOnly | Badge::Excluded);
    div()
        .rounded(px(5.))
        .border_1()
        .border_color(if bad { red(0.4) } else { content(0.12) })
        .px(px(6.))
        .py(px(1.))
        .text_size(px(11.))
        .line_height(px(15.))
        .text_color(if bad { red(0.9) } else { content(0.6) })
        .child(badge.label())
        .into_any_element()
}
