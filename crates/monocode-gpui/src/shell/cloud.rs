//! Cloud sessions in the shell: sessions that live in a GitHub codespace but
//! belong to a local project. They are listed in the local project's sidebar
//! with a cloud mark and the codespace's state, archive through a dialog that
//! asks what to do with the codespace, and open a local `gh codespace ssh`
//! terminal. All `gh` and network work runs on background threads, and state
//! is only ever read from the cache, so it never wakes a codespace.
use super::*;
use monocode_core::{
    cloud_sessions::{CloudSessions, CloudState, DeleteOutcome},
    codespaces::GhRunner,
};
use std::ffi::OsString;

/// How often the cached codespace states are re-read.
const REFRESH: std::time::Duration = std::time::Duration::from_secs(30);

/// A cloud session's binding: which local project it belongs to and where it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CloudMark {
    pub local_project: String,
    pub codespace: String,
}

/// What the sidebar shows for a codespace state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Good,
    Busy,
    Idle,
    Bad,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Indicator {
    pub label: &'static str,
    pub tone: Tone,
}

/// Row indicator for a codespace state. A state that could not be read (offline,
/// gh missing, not signed in) is "Not connected": a state, never an error toast.
pub(super) fn indicator(state: Option<CloudState>) -> Indicator {
    let (label, tone) = match state {
        Some(CloudState::Available) => ("Available", Tone::Good),
        Some(CloudState::Starting) => ("Starting", Tone::Busy),
        Some(CloudState::Stopping) => ("Stopping", Tone::Busy),
        Some(CloudState::Stopped) => ("Stopped", Tone::Idle),
        Some(CloudState::Unavailable) => ("Unavailable", Tone::Bad),
        Some(CloudState::Unknown) | None => ("Not connected", Tone::Idle),
    };
    Indicator { label, tone }
}

/// What a refresh found: bindings, cached states and the host's activity marks.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub marks: HashMap<i64, CloudMark>,
    pub states: HashMap<String, CloudState>,
    pub activity: HashMap<i64, (bool, bool)>,
    /// Whether each newly checked project has a GitHub `origin`.
    pub github: HashMap<String, bool>,
}

/// Reads every cloud session of `projects` from the store and the cached state of
/// each codespace. Blocking (a cache miss runs `gh codespace view`, which never
/// wakes a codespace): call it on a worker.
pub(super) fn collect(
    cloud: &CloudSessions,
    projects: &[String],
    unchecked: &[String],
) -> Snapshot {
    let mut snapshot = Snapshot::default();
    for project in unchecked.iter().filter(|p| !p.starts_with("remote://")) {
        snapshot
            .github
            .insert(project.clone(), cloud.repo_for(project).is_ok());
    }
    for project in projects.iter().filter(|p| !p.starts_with("remote://")) {
        let Ok(sessions) = cloud.cloud_sessions_for(project) else {
            continue;
        };
        for session in sessions {
            let id = session.summary.id;
            snapshot.marks.insert(
                id,
                CloudMark {
                    local_project: project.clone(),
                    codespace: session.codespace.clone(),
                },
            );
            snapshot
                .activity
                .insert(id, (session.running, session.needs_input));
            if let std::collections::hash_map::Entry::Vacant(slot) =
                snapshot.states.entry(session.codespace.clone())
            {
                slot.insert(cloud.cloud_state(&session.codespace));
            }
        }
    }
    snapshot
}

/// What the user can do with the codespace when archiving a cloud session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArchiveChoice {
    Stop,
    Delete,
    Keep,
}

/// The preselected choice: stopping is right unless other sessions still use
/// the codespace, and then it is left running.
pub(super) fn default_choice(others: usize) -> ArchiveChoice {
    if others > 0 {
        ArchiveChoice::Keep
    } else {
        ArchiveChoice::Stop
    }
}

pub(super) fn others_note(others: usize) -> Option<String> {
    match others {
        0 => None,
        1 => Some("1 other session uses this codespace.".into()),
        n => Some(format!("{n} other sessions use this codespace.")),
    }
}

/// What to do to the sessions once the codespace call succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Then {
    /// Archive the session as usual.
    Archive,
    /// The codespace is gone and every session bound to it with it.
    Forget,
}

/// Runs the codespace side of an archive. The second value is a warning to show
/// although the call took effect. Blocking: call it on a worker.
pub(super) fn apply_choice(
    cloud: &CloudSessions,
    choice: ArchiveChoice,
    codespace: &str,
) -> Result<(Then, Option<String>), String> {
    match choice {
        ArchiveChoice::Keep => Ok((Then::Archive, None)),
        ArchiveChoice::Stop => cloud
            .stop_codespace(codespace)
            .map(|()| (Then::Archive, None))
            .map_err(|e| format!("Could not stop the codespace: {e}")),
        // The codespace is gone and its bindings were dropped even when only
        // disconnecting the machine failed, so the sessions go either way.
        ArchiveChoice::Delete => match cloud.delete_codespace_outcome(codespace) {
            Ok(DeleteOutcome::Deleted) => Ok((Then::Forget, None)),
            Ok(DeleteOutcome::DeletedWithWarning(e)) => Ok((
                Then::Forget,
                Some(format!("Codespace deleted, but disconnecting failed: {e}")),
            )),
            Err(e) => Err(format!("Could not delete the codespace: {e}")),
        },
    }
}

/// Which sessions and codespaces the dialog is about.
pub(super) fn archive_summary(title: &str, groups: &[CodespaceGroup]) -> String {
    let sessions: usize = groups.iter().map(|g| g.ids.len()).sum();
    if let [group] = groups
        && sessions == 1
    {
        return format!("“{title}” runs in the codespace {}.", group.codespace);
    }
    let list = groups
        .iter()
        .map(|g| {
            format!(
                "{} ({} {})",
                g.codespace,
                g.ids.len(),
                if g.ids.len() == 1 {
                    "session"
                } else {
                    "sessions"
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{sessions} cloud sessions run in: {list}. The choice applies to each codespace.")
}

/// What the File and Git panels say while a cloud session is active.
pub(super) fn banner_text(codespace: &str) -> String {
    format!("Local files - this session runs in codespace {codespace}")
}

/// `gh codespace ssh -c <name>`: an interactive shell on the codespace.
pub(super) fn codespace_ssh_args(codespace: &str) -> Vec<OsString> {
    ["codespace", "ssh", "-c", codespace]
        .into_iter()
        .map(OsString::from)
        .collect()
}

/// `gh` on the login-shell PATH, as every other CLI the app runs.
fn gh_program() -> OsString {
    monocode_core::scm::resolve_gui_binary("gh")
        .map_or_else(|| OsString::from("gh"), std::path::PathBuf::into_os_string)
}

/// One codespace affected by an archive: the selected sessions in it and the
/// non-archived sessions outside the selection that still use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodespaceGroup {
    pub codespace: String,
    pub ids: Vec<i64>,
    pub others: usize,
}

/// Groups cloud sessions by codespace; `others_of` counts the other users.
pub(super) fn group_by_codespace(
    ids: &[i64],
    marks: &HashMap<i64, CloudMark>,
    others_of: impl Fn(&str, &[i64]) -> usize,
) -> Vec<CodespaceGroup> {
    let mut groups: Vec<CodespaceGroup> = Vec::new();
    for id in ids {
        let Some(mark) = marks.get(id) else { continue };
        match groups.iter_mut().find(|g| g.codespace == mark.codespace) {
            Some(group) => group.ids.push(*id),
            None => groups.push(CodespaceGroup {
                codespace: mark.codespace.clone(),
                ids: vec![*id],
                others: 0,
            }),
        }
    }
    for group in &mut groups {
        group.others = others_of(&group.codespace, &group.ids);
    }
    groups
}

pub(super) struct ArchiveDialog {
    pub(super) groups: Vec<CodespaceGroup>,
    pub(super) title: String,
    focus: FocusHandle,
}
impl ArchiveDialog {
    pub(super) fn ids(&self) -> Vec<i64> {
        self.groups.iter().flat_map(|g| g.ids.clone()).collect()
    }
    pub(super) fn others(&self) -> usize {
        self.groups.iter().map(|g| g.others).sum()
    }
}

pub(super) fn default_gh() -> Arc<dyn GhRunner> {
    Arc::new(monocode_core::codespaces::SystemGh)
}

pub(super) fn service(
    gh: Arc<dyn GhRunner>,
    remote: Arc<monocode_core::remote::Connections>,
    store: Database,
) -> Arc<CloudSessions> {
    Arc::new(CloudSessions::new(gh, Arc::new(remote), store))
}

pub(super) fn refresh_loop(cx: &mut Context<Shell>) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            if this.update(cx, |this, cx| this.refresh_cloud(cx)).is_err() {
                break;
            }
            cx.background_executor().timer(REFRESH).await;
        }
    })
}

impl Shell {
    /// The project a session is listed under: a cloud session belongs to the local
    /// project it twins, every other session to its own directory.
    pub(super) fn project_of<'a>(&'a self, id: i64, cwd: &'a str) -> &'a str {
        self.cloud_marks
            .get(&id)
            .map_or(cwd, |mark| mark.local_project.as_str())
    }

    /// Whether "New session in Codespace" applies to the project in view: a local
    /// folder whose `origin` is a GitHub repository.
    pub(super) fn offers_codespace(&self) -> bool {
        self.can_open_codespace()
            && self
                .github_projects
                .get(&self.workspace.project)
                .copied()
                .unwrap_or(false)
    }

    /// Marks a session the app or a Mono just created, so it is listed under its local project
    /// at once instead of after the next refresh. A no-op for a session that is not a cloud one.
    pub(super) fn mark_cloud_session(&mut self, id: i64) {
        let binding = locked(&self.store)
            .ok()
            .and_then(|store| store.remote_session(id).ok().flatten())
            .and_then(|remote| Some((remote.project.local_project?, remote.project.codespace?)));
        if let Some((local_project, codespace)) = binding {
            self.cloud_states
                .entry(codespace.clone())
                .or_insert(CloudState::Available);
            self.cloud_marks.insert(
                id,
                CloudMark {
                    local_project,
                    codespace,
                },
            );
        }
    }

    pub(super) fn is_cloud(&self, id: i64) -> bool {
        self.cloud_marks.contains_key(&id)
    }

    pub(super) fn refresh_cloud(&mut self, cx: &mut Context<Self>) {
        if self.cloud_refreshing {
            return;
        }
        self.cloud_refreshing = true;
        let projects = self.workspace.projects.clone();
        let unchecked: Vec<String> = projects
            .iter()
            .chain([&self.workspace.project])
            .filter(|p| !self.github_projects.contains_key(*p))
            .cloned()
            .collect();
        let cloud = self.cloud.clone();
        let work = cx
            .background_executor()
            .spawn(async move { collect(&cloud, &projects, &unchecked) });
        cx.spawn(async move |this, cx| {
            let snapshot = work.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud_refreshing = false;
                this.apply_cloud_snapshot(snapshot, cx);
            });
        })
        .detach();
    }

    pub(super) fn apply_cloud_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        let changed = snapshot.marks != self.cloud_marks || snapshot.states != self.cloud_states;
        for (id, activity) in &snapshot.activity {
            // Host activity of the project is only polled for remote projects.
            self.remote_marks.insert(*id, *activity);
        }
        self.github_projects.extend(snapshot.github);
        self.cloud_marks = snapshot.marks;
        self.cloud_states = snapshot.states;
        if changed {
            self.invalidate_chrome(cx);
            cx.notify();
        }
    }

    /// The cloud mark for a session row: a cloud glyph, the codespace state's dot
    /// and, in roomy rows, its name.
    pub(super) fn cloud_badge(&self, id: i64, compact: bool) -> Option<AnyElement> {
        let mark = self.cloud_marks.get(&id)?;
        let indicator = indicator(self.cloud_states.get(&mark.codespace).copied());
        let color = match indicator.tone {
            Tone::Good => rgb(0x4ade80).into(),
            Tone::Busy => rgb(0xfbbf24).into(),
            Tone::Bad => rgb(0xf87171).into(),
            Tone::Idle => content(0.35),
        };
        Some(
            div()
                .id(("cloud-badge", id as usize))
                .debug_selector(move || format!("cloud-badge-{id}"))
                .role(Role::Image)
                .aria_label(format!("Codespace {}: {}", mark.codespace, indicator.label))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(px(11.))
                .text_color(content(0.5))
                .child(icon("cloud-upload", 12.).text_color(content(0.55)))
                .child(div().size(px(6.)).rounded_full().bg(color))
                .when(!compact, |d| d.child(indicator.label))
                .into_any_element(),
        )
    }

    /// A local PTY running `gh codespace ssh`, rooted at the home directory.
    pub(super) fn spawn_codespace_terminal(
        &mut self,
        codespace: &str,
        cx: &mut Context<Self>,
    ) -> Entity<Terminal> {
        let cwd = crate::terminal::home_cwd();
        let terminal = cx.new(|cx| {
            Terminal::launch(
                std::path::Path::new(&cwd),
                gh_program(),
                codespace_ssh_args(codespace),
                Vec::new(),
                Vec::new(),
                cx,
            )
        });
        self.observations
            .push(cx.subscribe(&terminal, |_, _, _: &TerminalEvent, cx| cx.notify()));
        terminal
    }

    /// Terminal for a cloud session: a local PTY running `gh codespace ssh`.
    pub(super) fn new_codespace_terminal(
        &mut self,
        codespace: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = crate::terminal::home_cwd();
        let project = self.workspace.project.clone();
        let was_open = self
            .workspace
            .docks
            .get(&project)
            .is_some_and(|d| d.open && !d.terminals.is_empty());
        self.ensure_terminals(&project, cx);
        let viewport = self.viewport();
        let dock = self.workspace.docks.entry(project.clone()).or_default();
        dock.size = dock.side.clamp(dock.size, viewport);
        let added = dock.add(cwd.clone());
        self.dock_tab_scroll.scroll_to_item(added);
        let terminal = self.spawn_codespace_terminal(&codespace, cx);
        self.terminals.entry(project).or_default().push(terminal);
        if self.terminal_in_panel(cx) {
            if let Some(dock) = self.workspace.docks.get_mut(&self.workspace.project) {
                dock.open = false;
            }
            self.show_panel_terminal(window, cx);
            self.persist(cx);
            return;
        }
        if !was_open {
            self.slide_dock(true, cx);
        }
        self.focus_terminal(window, cx);
        self.fit_sidebar(cx);
        self.persist(cx);
        cx.notify();
    }

    /// While a cloud session is in view the File and Git panels still show the
    /// local project: say so, and keep git writes off it.
    pub(super) fn sync_cloud_panel(&self, cx: &mut Context<Self>) {
        let codespace = if self.mono_active(cx) {
            None
        } else {
            self.active_codespace()
        };
        let flipped = crate::git_gate::set(codespace.clone());
        if let Some(panel) = &self.right_panel {
            panel.update(cx, |panel, cx| {
                panel.set_cloud_banner(codespace.as_deref().map(banner_text), cx);
                if flipped {
                    panel
                        .changes
                        .update(cx, |changes, cx| changes.gate_changed(cx));
                }
            });
        }
        if flipped {
            for review in self.reviews.values() {
                review.update(cx, |_, cx| cx.notify());
            }
        }
    }

    /// The codespace of the active cloud session, when one is in view.
    pub(super) fn active_codespace(&self) -> Option<String> {
        let id = self.workspace.active?;
        self.cloud_marks.get(&id).map(|mark| mark.codespace.clone())
    }

    /// Archiving cloud sessions asks what to do with their codespaces first: one
    /// dialog for all of `ids`, grouped by codespace.
    pub(super) fn open_cloud_archive(
        &mut self,
        ids: Vec<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<i64> = ids
            .into_iter()
            .filter(|id| self.is_cloud(*id) && !self.cloud_busy.contains(id))
            .collect();
        if ids.is_empty() || self.cloud_archive.is_some() {
            return;
        }
        let groups = group_by_codespace(&ids, &self.cloud_marks, |codespace, selected| {
            self.cloud_marks
                .iter()
                .filter(|(other, m)| {
                    m.codespace == codespace
                        && !selected.contains(other)
                        && self
                            .session_summaries
                            .get(other)
                            .is_some_and(|s| !s.archived)
                })
                .count()
        });
        self.close_preview(cx);
        let title = match ids.as_slice() {
            [id] => self.session_metadata(*id, cx).display_title(),
            _ => String::new(),
        };
        let dialog = ArchiveDialog {
            groups,
            title,
            focus: cx.focus_handle(),
        };
        window.focus(&dialog.focus, cx);
        self.cloud_archive = Some(dialog);
        cx.notify();
    }

    pub(super) fn close_cloud_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud_archive.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(super) fn confirm_cloud_archive(
        &mut self,
        choice: ArchiveChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.cloud_archive.take() else {
            return;
        };
        self.focus_active(window, cx);
        if choice == ArchiveChoice::Keep {
            for id in dialog.ids() {
                self.toggle_archive_now(id, cx);
            }
            cx.notify();
            return;
        }
        let ids = dialog.ids();
        self.cloud_busy.extend(ids.iter().copied());
        let cloud = self.cloud.clone();
        let groups = dialog.groups.clone();
        let work = cx.background_executor().spawn(async move {
            groups
                .into_iter()
                .map(|group| {
                    let result = apply_choice(&cloud, choice, &group.codespace);
                    (group, result)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.cloud_busy.retain(|id| !ids.contains(id));
                let mut problems = Vec::new();
                for (group, result) in results {
                    match result {
                        Ok((then, warning)) => {
                            problems.extend(warning);
                            match then {
                                Then::Archive => {
                                    this.cloud_states
                                        .insert(group.codespace.clone(), CloudState::Stopping);
                                    for id in group.ids {
                                        this.toggle_archive_now(id, cx);
                                    }
                                }
                                Then::Forget => {
                                    this.forget_codespace_sessions(&group.codespace, window, cx)
                                }
                            }
                        }
                        Err(error) => problems.push(error),
                    }
                }
                if !problems.is_empty() {
                    this.error = Some(problems.join(" "));
                }
                this.invalidate_chrome(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Drops every session of a deleted codespace from the window.
    fn forget_codespace_sessions(
        &mut self,
        codespace: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gone: Vec<i64> = self
            .cloud_marks
            .iter()
            .filter(|(_, mark)| mark.codespace == codespace)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if self.workspace.tab_of(id).is_some() {
                self.close_pane(id, window, cx);
            }
            self.cloud_marks.remove(&id);
            self.remote_marks.remove(&id);
            self.forget_session(id, cx);
        }
        self.cloud_states.remove(codespace);
    }

    pub(super) fn cloud_archive_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.cloud_archive.as_ref()?;
        let default = default_choice(dialog.others());
        let width = 460f32.min(window.viewport_size().width.as_f32() - 24.);
        let weak = cx.weak_entity();
        let button = |id: &'static str,
                      label: &'static str,
                      choice: ArchiveChoice,
                      cx: &mut Context<Self>| {
            let base = if choice == ArchiveChoice::Delete {
                crate::modal::danger_button(id, label)
            } else if choice == default {
                crate::modal::primary_button(id, label)
            } else {
                crate::modal::ghost_button(id, label)
            };
            base.debug_selector(move || id.into())
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.confirm_cloud_archive(choice, window, cx)
                }))
        };
        Some(
            div()
                .absolute()
                .size_full()
                .child(crate::modal::scrim("cloud-archive-scrim"))
                .child(
                    popover("cloud-archive-modal", Anchor::Center)
                        .modal_surface(self.workspace.boxy_layout())
                        .width(width)
                        .gap(0.)
                        .priority(60)
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| this.close_cloud_archive(window, cx));
                        })
                        .child(
                            div()
                                .id("cloud-archive-dialog")
                                .role(Role::Dialog)
                                .aria_label("Archive cloud session")
                                .track_focus(&dialog.focus)
                                .key_context("CloudArchive")
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, window, cx| {
                                        if event.keystroke.key == "enter" {
                                            this.confirm_cloud_archive(default, window, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(1.))
                                        .child("Archive cloud session?"),
                                )
                                .child(div().text_size(px(12.)).line_height(px(17.)).child(
                                    format!(
                                        "{} A running codespace keeps costing money until it idles out.",
                                        archive_summary(&dialog.title, &dialog.groups)
                                    ),
                                ))
                                .children(others_note(dialog.others()).map(|note| {
                                    div()
                                        .id("cloud-archive-others")
                                        .text_size(px(12.))
                                        .line_height(px(17.))
                                        .text_color(content(0.6))
                                        .child(format!(
                                            "{note} Deleting the codespace removes those sessions from Monoptah too."
                                        ))
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            crate::modal::ghost_button("cloud-archive-cancel", "Cancel")
                                                .cursor_pointer()
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.close_cloud_archive(window, cx)
                                                })),
                                        )
                                        .child(button(
                                            "cloud-archive-keep",
                                            "Keep running",
                                            ArchiveChoice::Keep,
                                            cx,
                                        ))
                                        .child(button(
                                            "cloud-archive-delete",
                                            "Delete codespace",
                                            ArchiveChoice::Delete,
                                            cx,
                                        ))
                                        .child(button(
                                            "cloud-archive-stop",
                                            "Stop codespace",
                                            ArchiveChoice::Stop,
                                            cx,
                                        )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
