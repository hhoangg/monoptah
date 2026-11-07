//! Cloud sessions in the shell: sessions that live in a GitHub codespace but
//! belong to a local project. They are listed in the local project's sidebar
//! with a cloud mark and the codespace's state, archive through a dialog that
//! asks what to do with the codespace, and open a local `gh codespace ssh`
//! terminal. All `gh` and network work runs on background threads, and state
//! is only ever read from the cache, so it never wakes a codespace.
use super::*;
use monocode_core::{
    cloud_sessions::{
        CloudSessions, CloudState, Converted, DeleteOutcome, Restored, pending_restore_for,
        restore_code,
    },
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
    /// Converted local sessions and the codespace each came from.
    pub origins: HashMap<i64, String>,
    /// Codespaces among `origins` that gh says no longer exist.
    pub gone: std::collections::HashSet<String>,
}

/// Reads every cloud session of `projects` from the store and the cached state of
/// each codespace. Blocking (a cache miss runs `gh codespace view`, which never
/// wakes a codespace): call it on a worker.
pub(super) fn collect(
    cloud: &CloudSessions,
    projects: &[String],
    unchecked: &[String],
) -> Snapshot {
    let mut snapshot = Snapshot {
        origins: cloud.converted_origins().into_iter().collect(),
        ..Default::default()
    };
    let names: std::collections::BTreeSet<&String> = snapshot.origins.values().collect();
    snapshot.gone = names
        .into_iter()
        .filter(|name| cloud.codespace_gone(name))
        .cloned()
        .collect();
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

const TITLE_LIMIT: usize = 48;

/// The session title quoted for the dialog copy: single line, clipped with an
/// ellipsis when long, and "this session" when there is no title.
pub(super) fn quoted_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return "This session".into();
    }
    if title.chars().count() <= TITLE_LIMIT {
        return format!("“{title}”");
    }
    let clipped: String = title.chars().take(TITLE_LIMIT).collect();
    format!("“{}…”", clipped.trim_end())
}

/// Which sessions and codespaces the dialog is about.
pub(super) fn archive_summary(title: &str, groups: &[CodespaceGroup]) -> String {
    let sessions: usize = groups.iter().map(|g| g.ids.len()).sum();
    if let [group] = groups
        && sessions == 1
    {
        return format!(
            "{} runs in the codespace {}.",
            quoted_title(title),
            group.codespace
        );
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

/// Saves the local copy of one cloud session (see `CloudSessions::convert_session`).
pub(super) type ConvertFn = Arc<
    dyn Fn(&CloudSessions, i64, &mut dyn FnMut(&str)) -> Result<Converted, String> + Send + Sync,
>;

pub(super) fn default_convert() -> ConvertFn {
    Arc::new(|cloud, id, progress| cloud.convert_session(id, progress))
}

/// What one codespace's archive did.
pub(super) struct GroupOutcome {
    pub group: CodespaceGroup,
    /// Sessions whose local copy was saved (their cloud records are gone).
    pub converted: Vec<Converted>,
    /// The codespace call, or why nothing was done to it.
    pub result: Result<(Then, Option<String>), String>,
    /// With `result` set to `NEEDS_REVIEW`: what a delete would lose, one line each.
    pub risky: Vec<String>,
}

/// The error of a group that was paused before deleting its codespace: the local copy has files
/// missing that only exist there, so the user decides first.
pub(super) const NEEDS_REVIEW: &str = "Needs review";

pub(super) use monocode_core::cloud_sessions::unsaved_lines;

/// The worker side of Stop/Delete: for each codespace, save a local copy of every selected
/// cloud session that still exists, and only when all of them are saved stop or delete the
/// codespace. With `save_copy` false (the user chose "anyway") the copy is skipped. A failed
/// copy leaves the codespace alone. Sessions that were already converted by an earlier
/// attempt are skipped, so a retry never duplicates them.
#[allow(clippy::too_many_arguments)]
pub(super) fn archive_groups(
    cloud: &CloudSessions,
    groups: Vec<CodespaceGroup>,
    choice: ArchiveChoice,
    save_copy: bool,
    convert: &ConvertFn,
    cancel: &std::sync::atomic::AtomicBool,
    // What the user already saw and accepted losing; anything else asks again.
    confirmed: &[String],
    progress: &mut dyn FnMut(&str),
) -> Vec<GroupOutcome> {
    use std::sync::atomic::Ordering;
    // Deleting the codespace takes every session in it with it, archived or not, so all of
    // them are saved. Stopping only needs the selected ones.
    let work: Vec<Vec<i64>> = groups
        .iter()
        .map(|group| {
            let mut ids = group.ids.clone();
            if save_copy && choice == ArchiveChoice::Delete {
                for id in cloud.sessions_in_codespace(&group.codespace) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            ids
        })
        .collect();
    let total: usize = work.iter().map(Vec::len).sum();
    let mut done = 0;
    groups
        .into_iter()
        .zip(work)
        .map(|(group, ids)| {
            let mut converted = Vec::new();
            let mut failure = None;
            // Lines kept in memory too, so a failed store write cannot open the gate.
            let mut memory: Vec<String> = Vec::new();
            let mut store_warning = None;
            if save_copy {
                for id in &ids {
                    done += 1;
                    if cancel.load(Ordering::Relaxed) {
                        failure = Some(CANCELLED.to_string());
                        break;
                    }
                    if !cloud.is_cloud_session(*id) || cloud.is_pending_start(*id) {
                        continue;
                    }
                    let prefix = if total > 1 {
                        format!("({done}/{total}) ")
                    } else {
                        String::new()
                    };
                    match convert(cloud, *id, &mut |line| progress(&format!("{prefix}{line}"))) {
                        Ok(c) => {
                            // Remembered per codespace, so any later delete asks again.
                            let lines = unsaved_lines(std::slice::from_ref(&c));
                            if let Err(e) = cloud.record_unsaved(&group.codespace, &lines) {
                                store_warning = Some(format!(
                                    "Could not remember which files were not saved ({e}); deleting this codespace will still ask."
                                ));
                            }
                            memory.extend(lines);
                            converted.push(c)
                        }
                        Err(e) => {
                            failure = Some(format!("Could not save a local copy: {e}"));
                            break;
                        }
                    }
                }
            }
            if failure.is_none() && cancel.load(Ordering::Relaxed) {
                failure = Some(CANCELLED.to_string());
            }
            // Never delete silently while the codespace holds files the local copy lacks, even
            // when the copies were made by an earlier attempt: the list is stored.
            let mut risky = Vec::new();
            if failure.is_none() && choice == ArchiveChoice::Delete {
                let mut stored = cloud.unsaved_for(&group.codespace);
                for line in memory {
                    if !stored.contains(&line) {
                        stored.push(line);
                    }
                }
                if stored.iter().any(|line| !confirmed.contains(line)) {
                    risky = stored;
                    failure = Some(NEEDS_REVIEW.to_string());
                }
            }
            let result = match failure {
                Some(error) => Err(error),
                None => {
                    progress(match choice {
                        ArchiveChoice::Delete => "Deleting the codespace",
                        _ => "Stopping the codespace",
                    });
                    apply_choice(cloud, choice, &group.codespace).map(|(then, warning)| {
                        let joined = [warning, store_warning]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" ");
                        (then, (!joined.is_empty()).then_some(joined))
                    })
                }
            };
            GroupOutcome {
                group,
                converted,
                result,
                risky,
            }
        })
        .collect()
}

/// The error of a group the user cancelled before anything destructive happened.
pub(super) const CANCELLED: &str = "Cancelled";

/// What to tell the user after the code was restored.
pub(super) fn restored_message(done: &Restored) -> String {
    let place = format!(
        "Restored the codespace code on branch {} in {}.",
        done.branch,
        done.worktree.display()
    );
    match &done.conflicts {
        Some(conflicts) => format!(
            "{place} The uncommitted changes did not apply ({}); they are kept in the session folder.",
            conflicts.lines().next().unwrap_or("conflict")
        ),
        None => place,
    }
}

/// How many sessions "Delete codespace" saves: every session in the codespace.
pub(super) fn delete_note(groups: &[CodespaceGroup]) -> String {
    let total: usize = groups.iter().map(|g| g.total.max(g.ids.len())).sum();
    format!(
        "Delete codespace saves a local copy of all {total} {} in the {}.",
        if total == 1 { "session" } else { "sessions" },
        if groups.len() == 1 {
            "codespace"
        } else {
            "codespaces"
        },
    )
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
    /// Every cloud session the window knows in this codespace, archived or not: what Delete saves.
    pub total: usize,
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
                total: marks
                    .values()
                    .filter(|m| m.codespace == mark.codespace)
                    .count(),
            }),
        }
    }
    for group in &mut groups {
        group.others = others_of(&group.codespace, &group.ids);
    }
    groups
}

/// Where the archive dialog is: asking, saving the local copy, or waiting for a decision after
/// a failure.
pub(super) enum Phase {
    Choose,
    /// Saving a local copy; `progress` holds the latest line from the worker.
    Working {
        progress: Arc<std::sync::Mutex<String>>,
        /// Set by Cancel: the worker stops before the next copy and before stop/delete.
        cancel: Arc<std::sync::atomic::AtomicBool>,
    },
    /// The local copy (or the codespace call) failed; the user decides what next.
    Failed {
        choice: ArchiveChoice,
        error: String,
    },
    /// The local copies are saved, but deleting the codespace would lose what `items` says.
    Review {
        items: Vec<String>,
    },
    /// Deleting from the session menu: confirm first, listing what the codespace holds that the
    /// local copies lack.
    ConfirmDelete {
        items: Vec<String>,
    },
}

pub(super) struct ArchiveDialog {
    pub(super) groups: Vec<CodespaceGroup>,
    pub(super) title: String,
    pub(super) phase: Phase,
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
            // The session is listed under its local project, so that project must be open.
            self.workspace.add_project(&local_project);
            // The state is unknown until the next refresh reads it; never claim Available.
            self.cloud_states
                .entry(codespace.clone())
                .or_insert(CloudState::Unknown);
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
        let changed = snapshot.marks != self.cloud_marks
            || snapshot.states != self.cloud_states
            || snapshot.origins != self.cloud_origins
            || snapshot.gone != self.cloud_gone;
        self.cloud_origins = snapshot.origins;
        self.cloud_gone = snapshot.gone;
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
        let mut indicator = indicator(self.cloud_states.get(&mark.codespace).copied());
        // A startup in progress or failed replaces the codespace state: the dot is amber or red.
        if let Some(label) = self.cloud_start_label(id) {
            indicator = label;
        }
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
        self.sync_codespace_pane(cx);
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
            phase: Phase::Choose,
            focus: cx.focus_handle(),
        };
        window.focus(&dialog.focus, cx);
        self.cloud_archive = Some(dialog);
        cx.notify();
    }

    /// The codespace a converted local session came from, while gh says it still exists.
    pub(super) fn live_origin_codespace(&self, id: i64) -> Option<String> {
        self.cloud_origins
            .get(&id)
            .filter(|name| !self.cloud_gone.contains(*name) && !self.is_cloud(id))
            .cloned()
    }

    /// Stops or deletes the codespace a converted session came from (session menu). Deleting
    /// saves any session still in it and asks first when files would be lost.
    pub(super) fn run_origin_codespace_action(
        &mut self,
        id: i64,
        choice: ArchiveChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(codespace) = self.live_origin_codespace(id) else {
            return;
        };
        if self.cloud_archive.is_some() {
            return;
        }
        let total = self
            .cloud_marks
            .values()
            .filter(|m| m.codespace == codespace)
            .count();
        self.close_preview(cx);
        self.cloud_archive = Some(ArchiveDialog {
            groups: vec![CodespaceGroup {
                codespace: codespace.clone(),
                ids: vec![],
                others: 0,
                total,
            }],
            title: String::new(),
            phase: Phase::Choose,
            focus: cx.focus_handle(),
        });
        if choice == ArchiveChoice::Delete {
            let items = self.cloud.unsaved_for(&codespace);
            if let Some(dialog) = self.cloud_archive.as_mut() {
                dialog.phase = Phase::ConfirmDelete { items };
                window.focus(&dialog.focus, cx);
            }
            cx.notify();
            return;
        }
        self.run_cloud_archive(choice, false, Vec::new(), window, cx);
    }

    pub(super) fn close_cloud_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Saving a local copy cannot be interrupted half way: Cancel asks the worker to stop
        // before its next step, and the dialog closes when it has.
        if let Some(Phase::Working { cancel, .. }) = self.cloud_archive.as_ref().map(|d| &d.phase) {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            return;
        }
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
        if self
            .cloud_archive
            .as_ref()
            .is_none_or(|d| !matches!(d.phase, Phase::Choose))
        {
            return;
        }
        if choice == ArchiveChoice::Keep {
            let Some(dialog) = self.cloud_archive.take() else {
                return;
            };
            self.focus_active(window, cx);
            for id in dialog.ids() {
                self.toggle_archive_now(id, cx);
            }
            cx.notify();
            return;
        }
        self.run_cloud_archive(choice, true, Vec::new(), window, cx);
    }

    /// Saves a local copy of every session (unless `save_copy` is false), then stops or
    /// deletes the codespace. The dialog stays open and shows the progress; a failure turns
    /// it into the Retry / anyway / Cancel prompt.
    pub(super) fn run_cloud_archive(
        &mut self,
        choice: ArchiveChoice,
        save_copy: bool,
        confirmed: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.cloud_archive.as_mut() else {
            return;
        };
        let progress = Arc::new(std::sync::Mutex::new(String::from("Starting")));
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        dialog.phase = Phase::Working {
            progress: progress.clone(),
            cancel: cancel.clone(),
        };
        let groups = dialog.groups.clone();
        let ids = dialog.ids();
        window.focus(&dialog.focus, cx);
        self.cloud_busy.extend(ids.iter().copied());
        // Delete saves every session of the codespace, so all of them stop taking messages.
        let mut locked_ids = ids.clone();
        if choice == ArchiveChoice::Delete {
            for group in &groups {
                locked_ids.extend(
                    self.cloud_marks
                        .iter()
                        .filter(|(_, m)| m.codespace == group.codespace)
                        .map(|(id, _)| *id),
                );
            }
        }
        self.lock_cloud_composer(&locked_ids, true, cx);
        let cloud = self.cloud.clone();
        let convert = self.cloud_convert.clone();
        let worker_progress = progress.clone();
        let work = cx.background_executor().spawn(async move {
            archive_groups(
                &cloud,
                groups,
                choice,
                save_copy,
                &convert,
                &cancel,
                &confirmed,
                &mut |line| {
                    if let Ok(mut current) = worker_progress.lock() {
                        *current = line.to_string();
                    }
                },
            )
        });
        // The worker cannot touch the window, so the dialog polls its progress line.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let live = this
                    .update(cx, |this, cx| {
                        let working = this
                            .cloud_archive
                            .as_ref()
                            .is_some_and(|d| matches!(d.phase, Phase::Working { .. }));
                        if working {
                            cx.notify();
                        }
                        working
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let outcomes = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.lock_cloud_composer(&locked_ids, false, cx);
                this.finish_cloud_archive(choice, ids, outcomes, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    /// Stops (or resumes) message sending in the open conversations of `ids`.
    pub(super) fn lock_cloud_composer(
        &mut self,
        ids: &[i64],
        locked: bool,
        cx: &mut Context<Self>,
    ) {
        for id in ids {
            if let Some(view) = self.sessions.get(id) {
                view.update(cx, |view, _| view.cloud_locked = locked);
            }
        }
    }

    fn finish_cloud_archive(
        &mut self,
        choice: ArchiveChoice,
        ids: Vec<i64>,
        outcomes: Vec<GroupOutcome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cloud_busy.retain(|id| !ids.contains(id));
        // Conversion warnings are notices; a partial codespace failure stays an error.
        let mut notes = Vec::new();
        let mut warnings = Vec::new();
        let mut failed: Vec<(CodespaceGroup, String)> = Vec::new();
        let mut review: Vec<(CodespaceGroup, Vec<String>)> = Vec::new();
        for outcome in outcomes {
            let GroupOutcome {
                mut group,
                converted,
                result,
                risky,
            } = outcome;
            for saved in converted {
                warnings.extend(saved.warnings.iter().cloned());
                self.adopt_converted(saved, window, cx);
            }
            group.ids.retain(|id| self.is_cloud(*id));
            match result {
                Ok((then, warning)) => {
                    notes.extend(warning);
                    match then {
                        Then::Archive => {
                            self.cloud_states
                                .insert(group.codespace.clone(), CloudState::Stopping);
                            for id in group.ids {
                                self.toggle_archive_now(id, cx);
                            }
                        }
                        Then::Forget => {
                            self.cloud_gone.insert(group.codespace.clone());
                            self.forget_codespace_sessions(&group.codespace, window, cx)
                        }
                    }
                }
                Err(error) if error == CANCELLED => {}
                Err(error) if error == NEEDS_REVIEW => review.push((group, risky)),
                Err(error) => failed.push((group, error)),
            }
        }
        if failed.is_empty() && review.is_empty() {
            if self.cloud_archive.take().is_some() {
                self.focus_active(window, cx);
            }
        } else if let Some(dialog) = self.cloud_archive.as_mut() {
            if failed.is_empty() {
                let items = review.iter().flat_map(|(_, items)| items.clone()).collect();
                dialog.groups = review.into_iter().map(|(group, _)| group).collect();
                dialog.phase = Phase::Review { items };
            } else {
                let error = failed
                    .iter()
                    .map(|(_, error)| error.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                // Paused groups ride along: a retry asks about them again before deleting,
                // from the list stored for their codespace.
                dialog.groups = failed
                    .into_iter()
                    .map(|(group, _)| group)
                    .chain(review.into_iter().map(|(group, _)| group))
                    .collect();
                dialog.phase = Phase::Failed { choice, error };
            }
            window.focus(&dialog.focus, cx);
        }
        if !notes.is_empty() {
            self.error = Some(notes.join(" "));
        }
        if !warnings.is_empty() {
            self.notice = Some(warnings.join(" "));
        }
        self.invalidate_chrome(cx);
        cx.notify();
    }

    /// Whether local session `id` came from a codespace and its code was not restored yet.
    pub(super) fn codespace_restore_pending(&self, id: i64) -> bool {
        !self.cloud_busy.contains(&id)
            && locked(&self.store).is_ok_and(|store| pending_restore_for(&store, id).is_some())
    }

    /// Recreates the codespace's code in a new git worktree of the session's project.
    pub(super) fn restore_codespace_code(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.codespace_restore_pending(id) {
            return;
        }
        self.cloud_busy.insert(id);
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let (project, thread) = {
                let store = locked(&store)?;
                let project = store
                    .sessions()?
                    .into_iter()
                    .find(|s| s.id == id)
                    .ok_or("Session not found")?
                    .cwd;
                let thread = store
                    .thread_dir(id)?
                    .ok_or("This store keeps no session folders.")?;
                (project, thread)
            };
            restore_code(std::path::Path::new(&project), &thread)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud_busy.remove(&id);
                this.error = Some(match result {
                    Ok(done) => restored_message(&done),
                    Err(error) => format!("Could not restore the code: {error}"),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Replaces a converted cloud session in the window by its archived local copy.
    fn adopt_converted(&mut self, saved: Converted, window: &mut Window, cx: &mut Context<Self>) {
        let cloud_id = saved.cloud_id;
        if self.workspace.tab_of(cloud_id).is_some() {
            self.close_pane(cloud_id, window, cx);
        }
        self.cloud_marks.remove(&cloud_id);
        self.remote_marks.remove(&cloud_id);
        self.forget_session(cloud_id, cx);
        let id = saved.local_id;
        self.cloud_origins.insert(id, saved.codespace.clone());
        self.workspace.add_project(&saved.summary.cwd);
        self.session_summaries.insert(id, saved.summary);
        self.order.retain(|s| *s != id);
        self.order.insert(0, id);
    }

    /// Drops every session of a deleted codespace from the window.
    pub(super) fn forget_codespace_sessions(
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
        let width = 560f32.min(window.viewport_size().width.as_f32() - 24.);
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
        let cancel = crate::modal::ghost_button("cloud-archive-cancel", "Cancel")
            .debug_selector(|| "cloud-archive-cancel".into())
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.close_cloud_archive(window, cx)));
        let text = |id: &'static str, text: String, dim: bool| {
            div()
                .id(id)
                .text_size(px(12.))
                .line_height(px(17.))
                .when(dim, |d| d.text_color(content(0.6)))
                .child(text)
        };
        let footer = div()
            .flex()
            .flex_nowrap()
            .items_center()
            .justify_end()
            .gap_2();
        let (heading, body, footer) = match &dialog.phase {
            Phase::Choose => (
                "Archive cloud session?",
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(text(
                        "cloud-archive-summary",
                        format!(
                            "{} A running codespace keeps costing money until it idles out. \
                             Stopping or deleting it first saves an archived local copy with the \
                             conversation, evidence and code (branches, the stash and uncommitted \
                             changes; ignored files are listed but not saved). {}",
                            archive_summary(&dialog.title, &dialog.groups),
                            delete_note(&dialog.groups),
                        ),
                        false,
                    ))
                    .children(others_note(dialog.others()).map(|note| {
                        text(
                            "cloud-archive-others",
                            format!(
                                "{note} Deleting the codespace removes those sessions from Monoptah too."
                            ),
                            true,
                        )
                    })),
                footer
                    .child(cancel)
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
            Phase::Working { progress, .. } => (
                "Saving a local copy",
                div().child(text(
                    "cloud-archive-progress",
                    format!(
                        "{}...",
                        progress
                            .lock()
                            .map(|line| line.clone())
                            .unwrap_or_default()
                            .trim_end_matches(['.', '\u{2026}'])
                    ),
                    false,
                )),
                footer.child(cancel),
            ),
            Phase::ConfirmDelete { items } => {
                let seen = items.clone();
                let name = dialog
                    .groups
                    .first()
                    .map(|g| g.codespace.clone())
                    .unwrap_or_default();
                let mut lines = vec![format!(
                    "Delete the codespace {name}? This cannot be undone."
                )];
                lines.extend(items.iter().cloned());
                (
                    "Delete codespace?",
                    div().child(text("cloud-archive-confirm", lines.join(" "), false)),
                    footer
                        .child(cancel)
                        .child(
                            crate::modal::danger_button(
                                "cloud-archive-confirm-delete",
                                "Delete codespace",
                            )
                            .debug_selector(|| "cloud-archive-confirm-delete".into())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_cloud_archive(
                                    ArchiveChoice::Delete,
                                    true,
                                    seen.clone(),
                                    window,
                                    cx,
                                )
                            })),
                        ),
                )
            }
            Phase::Review { items } => {
                let seen = items.clone();
                (
                    "Some files are not saved",
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(text("cloud-archive-risk", items.join(" "), false))
                        .child(text(
                            "cloud-archive-risk-note",
                            "The local copy is saved without them. Deleting the codespace removes \
                             them for good; stopping it keeps them. Keep running leaves it as it \
                             is: stop or delete it later from the session menu."
                                .into(),
                            true,
                        )),
                    footer
                        .child(
                            crate::modal::ghost_button("cloud-archive-review-keep", "Keep running")
                                .debug_selector(|| "cloud-archive-review-keep".into())
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.close_cloud_archive(window, cx)
                                    }),
                                ),
                        )
                        .child(
                            crate::modal::danger_button(
                                "cloud-archive-delete-anyway",
                                "Delete anyway",
                            )
                            .debug_selector(|| "cloud-archive-delete-anyway".into())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // Copies any session that appeared meanwhile, then asks again
                                // if that found more to lose.
                                this.run_cloud_archive(
                                    ArchiveChoice::Delete,
                                    true,
                                    seen.clone(),
                                    window,
                                    cx,
                                )
                            })),
                        )
                        .child(
                            crate::modal::primary_button(
                                "cloud-archive-stop-instead",
                                "Stop instead",
                            )
                            .debug_selector(|| "cloud-archive-stop-instead".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run_cloud_archive(
                                    ArchiveChoice::Stop,
                                    false,
                                    Vec::new(),
                                    window,
                                    cx,
                                )
                            })),
                        ),
                )
            }
            Phase::Failed { choice, error } => {
                let choice = *choice;
                let anyway = if choice == ArchiveChoice::Delete {
                    "Delete anyway"
                } else {
                    "Stop anyway"
                };
                (
                    "Could not finish",
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(text("cloud-archive-error", error.clone(), false))
                        .child(text(
                            "cloud-archive-warning",
                            "Going on without a complete local copy can lose the code and files \
                             that only exist on the codespace."
                                .into(),
                            true,
                        )),
                    footer
                        .child(cancel)
                        .child(
                            crate::modal::danger_button("cloud-archive-anyway", anyway)
                                .debug_selector(|| "cloud-archive-anyway".into())
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_cloud_archive(choice, false, Vec::new(), window, cx)
                                })),
                        )
                        .child(
                            crate::modal::primary_button("cloud-archive-retry", "Retry")
                                .debug_selector(|| "cloud-archive-retry".into())
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_cloud_archive(choice, true, Vec::new(), window, cx)
                                })),
                        ),
                )
            }
        };
        let choosing = matches!(dialog.phase, Phase::Choose);
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
                            let _ =
                                weak.update(cx, |this, cx| this.close_cloud_archive(window, cx));
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
                                        if choosing && event.keystroke.key == "enter" {
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
                                        .child(heading),
                                )
                                .child(body)
                                .child(footer),
                        ),
                )
                .into_any_element(),
        )
    }
}
