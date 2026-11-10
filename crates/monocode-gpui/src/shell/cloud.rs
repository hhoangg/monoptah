//! Cloud sessions in the shell: sessions that live in a GitHub codespace (or a Docker
//! sandbox, worded with its own noun) but belong to a local project. They are listed in the
//! local project's sidebar with a cloud mark and the codespace's state, archive through a dialog that
//! asks what to do with the codespace, and open a local `gh codespace ssh`
//! terminal. All `gh` and network work runs on background threads, and state
//! is only ever read from the cache, so it never wakes a codespace.
use super::*;
use monocode_core::{
    cloud_sessions::{
        CloudBackendKind, CloudRef, CloudSessions, CloudState, ConvertError, Converted,
        DeleteOutcome, Restored, SandboxTerminal, pending_restore_for, restore_code,
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
    /// Name of the workspace: a codespace name or a sandbox name, see [`Self::kind`].
    pub codespace: String,
    pub kind: CloudBackendKind,
}

impl CloudMark {
    pub(super) fn cloud(&self) -> CloudRef {
        CloudRef {
            kind: self.kind,
            name: self.codespace.clone(),
        }
    }
    /// Key of the cached state (`Shell::cloud_states`).
    pub(super) fn key(&self) -> String {
        self.cloud().key()
    }
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
    /// By [`CloudRef::key`]: the bare name for a codespace.
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
                    codespace: session.cloud.name.clone(),
                    kind: session.cloud.kind,
                },
            );
            snapshot
                .activity
                .insert(id, (session.running, session.needs_input));
            if let std::collections::hash_map::Entry::Vacant(slot) =
                snapshot.states.entry(session.cloud.key())
            {
                slot.insert(cloud.cloud_state_of(&session.cloud));
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

pub(super) fn others_note(others: usize, noun: &str) -> Option<String> {
    match others {
        0 => None,
        1 => Some(format!("1 other session uses this {noun}.")),
        n => Some(format!("{n} other sessions use this {noun}.")),
    }
}

/// The words the archive dialog uses for its groups: the backend's own when they all share
/// one, "workspace" when a selection mixes codespaces and sandboxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Words {
    pub noun: &'static str,
    pub title: &'static str,
    pub plural: &'static str,
    kind: Option<CloudBackendKind>,
}

pub(super) fn words(groups: &[CodespaceGroup]) -> Words {
    let kind = groups
        .first()
        .map(|g| g.kind)
        .filter(|kind| groups.iter().all(|g| g.kind == *kind));
    match kind {
        Some(CloudBackendKind::DockerSandbox) => Words {
            noun: "sandbox",
            title: "Sandbox",
            plural: "sandboxes",
            kind,
        },
        Some(CloudBackendKind::Codespaces) => Words {
            noun: "codespace",
            title: "Codespace",
            plural: "codespaces",
            kind,
        },
        None => Words {
            noun: "workspace",
            title: "Workspace",
            plural: "workspaces",
            kind: None,
        },
    }
}

/// The question of the delete confirmation (session menu).
pub(super) fn delete_confirm_line(words: &Words, name: &str) -> String {
    format!("Delete the {} {name}? This cannot be undone.", words.noun)
}

/// For a stopped sandbox: saving the local copy before a delete has to reach into it.
pub(super) fn delete_start_note(words: &Words, stopped: bool) -> Option<String> {
    (stopped && words.kind == Some(CloudBackendKind::DockerSandbox)).then(|| {
        "Deleting saves a local copy first, which starts the sandbox for a moment.".to_string()
    })
}

/// The body of the "Remove from Monoptah" confirmation.
pub(super) fn remove_confirm_body(title: &str, name: &str) -> String {
    format!(
        "Remove {} from Monoptah? The sandbox {name} keeps existing and keeps costing money \
         until you delete it (`sbx --cloud rm {name}`). Its conversation is not saved on this \
         computer.",
        quoted_title_single(title),
    )
}

/// Like [`quoted_title`] with single curly quotes, as the other confirmations quote titles.
fn quoted_title_single(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return "this session".into();
    }
    if title.chars().count() <= TITLE_LIMIT {
        return format!("\u{2018}{title}\u{2019}");
    }
    let clipped: String = title.chars().take(TITLE_LIMIT).collect();
    format!("\u{2018}{}\u{2026}\u{2019}", clipped.trim_end())
}

/// The note under the list of files the local copy lacks.
pub(super) fn review_note(words: &Words) -> String {
    format!(
        "The local copy is saved without them. Deleting the {} removes them for good; \
         stopping it keeps them. Keep running leaves it as it is: stop or delete it later \
         from the session menu.",
        words.noun
    )
}

/// The warning under a failed save or stop.
pub(super) fn failed_note(words: &Words) -> String {
    format!(
        "Going on without a complete local copy can lose the code and files that only exist \
         on the {}.",
        words.noun
    )
}

impl Words {
    /// What a running workspace costs, as the dialog warns about it.
    pub(super) fn cost(&self) -> String {
        match self.kind {
            Some(CloudBackendKind::DockerSandbox) => {
                "A running sandbox keeps costing money until its runtime limit stops it.".into()
            }
            Some(CloudBackendKind::Codespaces) => {
                "A running codespace keeps costing money until it idles out.".into()
            }
            None => "A running workspace keeps costing money until it idles out or its runtime \
                     limit stops it."
                .into(),
        }
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
    target: &CloudRef,
) -> Result<(Then, Option<String>), String> {
    match choice {
        ArchiveChoice::Keep => Ok((Then::Archive, None)),
        ArchiveChoice::Stop => cloud
            .stop_cloud(target)
            .map(|()| (Then::Archive, None))
            .map_err(|e| format!("Could not stop the {}: {e}", target.noun())),
        // The workspace is gone and its bindings were dropped even when only
        // disconnecting the machine failed, so the sessions go either way.
        ArchiveChoice::Delete => match cloud.delete_cloud_outcome(target) {
            Ok(DeleteOutcome::Deleted) => Ok((Then::Forget, None)),
            Ok(DeleteOutcome::DeletedWithWarning(e)) => Ok((
                Then::Forget,
                Some(format!(
                    "{} deleted, but disconnecting failed: {e}",
                    target.title()
                )),
            )),
            Err(e) => Err(format!("Could not delete the {}: {e}", target.noun())),
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
    let words = words(groups);
    if let [group] = groups
        && sessions == 1
    {
        return format!(
            "{} runs in the {} {}.",
            quoted_title(title),
            words.noun,
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
    format!(
        "{sessions} cloud sessions run in: {list}. The choice applies to each {}.",
        words.noun
    )
}

/// Saves the local copy of one cloud session (see `CloudSessions::convert_session`).
/// The `bool` is whether the user already agreed to archive files only (see
/// [`ConvertError::NeedsFilesOnlyConfirm`]).
pub(super) type ConvertFn = Arc<
    dyn Fn(&CloudSessions, i64, bool, &mut dyn FnMut(&str)) -> Result<Converted, ConvertError>
        + Send
        + Sync,
>;

pub(super) fn default_convert() -> ConvertFn {
    Arc::new(|cloud, id, files_only, progress| {
        cloud.convert_session_confirmed(id, files_only, progress)
    })
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
    /// With `result` set to `NEEDS_FILES_ONLY`: the session whose conversation could not be
    /// reached and why.
    pub unreachable: Option<(i64, String)>,
}

/// The error of a group that was paused before deleting its codespace: the local copy has files
/// missing that only exist there, so the user decides first.
pub(super) const NEEDS_REVIEW: &str = "Needs review";

/// The error of a group paused because a conversation could not be reached: the user decides
/// between retrying and archiving only the code.
pub(super) const NEEDS_FILES_ONLY: &str = "Needs files-only confirmation";

/// The body of the "Archive files only?" step.
pub(super) fn files_only_body(title: &str, reason: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let who = if title.is_empty() {
        "this session".to_string()
    } else if title.chars().count() <= TITLE_LIMIT {
        format!("\u{2018}{title}\u{2019}")
    } else {
        let clipped: String = title.chars().take(TITLE_LIMIT).collect();
        format!("\u{2018}{}\u{2026}\u{2019}", clipped.trim_end())
    };
    format!(
        "The conversation of {who} could not be reached ({reason}). You can retry, or save \
         only the code (unpushed commits and uncommitted changes) and continue. The \
         conversation will not be saved."
    )
}

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
    // Sessions whose user agreed to archive only the code.
    files_only: &[i64],
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
                for id in cloud.sessions_in_cloud(&group.cloud()) {
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
            // Never try to back up a Codespace which GitHub confirms is gone.
            // Lookup failures keep the group intact, including on "delete anyway".
            let preflight = if cancel.load(Ordering::Relaxed) {
                Err(CANCELLED.to_string())
            } else if choice == ArchiveChoice::Delete {
                cloud.forget_missing(&group.cloud())
            } else {
                Ok(false)
            };
            match preflight {
                Ok(true) => {
                    return GroupOutcome {
                        group,
                        converted,
                        result: Ok((Then::Forget, None)),
                        risky: Vec::new(),
                        unreachable: None,
                    };
                }
                Err(error) => {
                    return GroupOutcome {
                        group,
                        converted,
                        result: Err(error),
                        risky: Vec::new(),
                        unreachable: None,
                    };
                }
                Ok(false) => {}
            }
            let mut failure = None;
            let mut unreachable = None;
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
                    let agreed = files_only.contains(id);
                    match convert(cloud, *id, agreed, &mut |line| {
                        progress(&format!("{prefix}{line}"))
                    }) {
                        Ok(c) => {
                            // Remembered per codespace, so any later delete asks again.
                            let lines = unsaved_lines(std::slice::from_ref(&c));
                            if let Err(e) = cloud.record_unsaved_cloud(&group.cloud(), &lines) {
                                store_warning = Some(format!(
                                    "Could not remember which files were not saved ({e}); deleting this {} will still ask.",
                                    group.kind.noun()
                                ));
                            }
                            memory.extend(lines);
                            converted.push(c)
                        }
                        Err(ConvertError::NeedsFilesOnlyConfirm { reason }) => {
                            failure = Some(NEEDS_FILES_ONLY.to_string());
                            unreachable = Some((*id, reason));
                            break;
                        }
                        Err(ConvertError::Failed(e)) => {
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
                let mut stored = cloud.unsaved_for_cloud(&group.cloud());
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
                    progress(&match choice {
                        ArchiveChoice::Delete => format!("Deleting the {}", group.kind.noun()),
                        _ => format!("Stopping the {}", group.kind.noun()),
                    });
                    apply_choice(cloud, choice, &group.cloud()).map(|(then, warning)| {
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
                unreachable,
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
    let words = words(groups);
    format!(
        "Delete {} saves a local copy of all {total} {} in the {}.",
        words.noun,
        if total == 1 { "session" } else { "sessions" },
        if groups.len() == 1 {
            words.noun
        } else {
            words.plural
        },
    )
}

/// `ssh -t <id>@sbx_cloud <devcontainer exec ... bash -l>`: an interactive shell in a
/// sandbox's container (a sandbox without a devcontainer: `cd <workspace> && bash -l`). The
/// remote command is one word, as the start flow sends it.
pub(super) fn sandbox_ssh_args(terminal: &SandboxTerminal) -> Vec<OsString> {
    [
        "-t",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-o",
        "PermitLocalCommand=no",
        // Resuming a stopped sandbox through the sbx ProxyCommand can take a while.
        "-o",
        "ConnectTimeout=120",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "--",
        terminal.destination.as_str(),
        terminal.remote.as_str(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

/// `ssh` on the login-shell PATH.
fn ssh_program() -> OsString {
    monocode_core::scm::resolve_gui_binary("ssh")
        .map_or_else(|| OsString::from("ssh"), std::path::PathBuf::into_os_string)
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
    /// Name of the workspace (see [`CloudMark::codespace`]).
    pub codespace: String,
    pub kind: CloudBackendKind,
    pub ids: Vec<i64>,
    pub others: usize,
    /// Every cloud session the window knows in this codespace, archived or not: what Delete saves.
    pub total: usize,
}

impl CodespaceGroup {
    pub(super) fn cloud(&self) -> CloudRef {
        CloudRef {
            kind: self.kind,
            name: self.codespace.clone(),
        }
    }
}

/// Groups cloud sessions by codespace; `others_of` counts the other users.
pub(super) fn group_by_codespace(
    ids: &[i64],
    marks: &HashMap<i64, CloudMark>,
    others_of: impl Fn(&CloudRef, &[i64]) -> usize,
) -> Vec<CodespaceGroup> {
    let mut groups: Vec<CodespaceGroup> = Vec::new();
    for id in ids {
        let Some(mark) = marks.get(id) else { continue };
        match groups
            .iter_mut()
            .find(|g| g.codespace == mark.codespace && g.kind == mark.kind)
        {
            Some(group) => group.ids.push(*id),
            None => groups.push(CodespaceGroup {
                codespace: mark.codespace.clone(),
                kind: mark.kind,
                ids: vec![*id],
                others: 0,
                total: marks
                    .values()
                    .filter(|m| m.codespace == mark.codespace && m.kind == mark.kind)
                    .count(),
            }),
        }
    }
    for group in &mut groups {
        group.others = others_of(&group.cloud(), &group.ids);
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
    /// "Remove from Monoptah" of session `id` (a stopped sandbox's): confirm first.
    ConfirmRemove {
        id: i64,
        title: String,
        name: String,
    },
    /// The conversation of session `id` could not be reached: retry, or archive only the code.
    FilesOnly {
        choice: ArchiveChoice,
        id: i64,
        title: String,
        reason: String,
    },
    /// The local copies are saved, but deleting the codespace would lose what `items` says.
    Review {
        items: Vec<String>,
    },
    /// Stopping from the session menu while an agent turn is running in the workspace: confirm
    /// first, since stopping interrupts it.
    ConfirmStop,
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
    /// Sessions whose user agreed to archive only the code.
    pub(super) files_only: Vec<i64>,
    /// Groups whose conversation could not be reached while another group failed: they get
    /// their own "Archive files only?" step once the failed ones are dealt with, so a "Delete
    /// anyway" never reaches them unseen.
    deferred: Vec<Unreachable>,
    focus: FocusHandle,
}

/// Shown when a second archive, stop or delete targets a workspace that one is working on.
pub(super) const WORKSPACE_BUSY: &str = "Another archive, stop or delete of this workspace is still running. Try again when it finishes.";

/// What is left for a dialog to ask after [`Shell::settle_cloud_archive`].
pub(super) struct Settled {
    pub(super) failed: Vec<(CodespaceGroup, String)>,
    pub(super) review: Vec<(CodespaceGroup, Vec<String>)>,
    unreachable: Vec<Unreachable>,
}

/// A paused group and what the files-only step says about it.
#[derive(Clone)]
struct Unreachable {
    group: CodespaceGroup,
    id: i64,
    title: String,
    reason: String,
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
    let cloud = CloudSessions::new(gh, Arc::new(remote), store.clone());
    // The app registers the real Docker sandbox backend (sbx, ssh, its own secret store).
    // Tests inject fakes through their own constructors and never reach the real `sbx`.
    #[cfg(not(test))]
    let cloud = cloud.with_system_sandboxes(store);
    #[cfg(test)]
    drop(store);
    Arc::new(cloud)
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
            .and_then(|remote| {
                Some((
                    remote.project.local_project.clone()?,
                    remote.project.cloud()?,
                ))
            });
        if let Some((local_project, cloud)) = binding {
            // The session is listed under its local project, so that project must be open.
            self.workspace.add_project(&local_project);
            // The state is unknown until the next refresh reads it; never claim Available.
            self.cloud_states
                .entry(cloud.key())
                .or_insert(CloudState::Unknown);
            self.cloud_marks.insert(
                id,
                CloudMark {
                    local_project,
                    codespace: cloud.name,
                    kind: cloud.kind,
                },
            );
        }
    }

    pub(super) fn is_cloud(&self, id: i64) -> bool {
        self.cloud_marks.contains_key(&id)
    }

    /// The user resumed the workspace of session `id`: it is no longer Stopped, whatever an
    /// older refresh says. The cached state flips at once (hiding the banner and the menu entry)
    /// and the real state is read again, after the refresh in flight if there is one.
    pub(super) fn cloud_resumed(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(mark) = self.cloud_marks.get(&id) {
            self.cloud_states.insert(mark.key(), CloudState::Available);
        }
        self.sync_cloud_stopped(cx);
        self.invalidate_chrome(cx);
        if self.cloud_refreshing {
            self.cloud_refresh_again = true;
        } else {
            self.refresh_cloud(cx);
        }
        cx.notify();
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
            let _ = this.update(cx, |this, cx| this.finish_cloud_refresh(snapshot, cx));
        })
        .detach();
    }

    /// A refresh answered. When a resume happened while it ran, it may still say Stopped: it is
    /// dropped and the state is read again.
    pub(super) fn finish_cloud_refresh(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.cloud_refreshing = false;
        if std::mem::take(&mut self.cloud_refresh_again) {
            self.refresh_cloud(cx);
        } else {
            self.apply_cloud_snapshot(snapshot, cx);
        }
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
        self.sync_codespace_offers(cx);
        self.cloud_marks = snapshot.marks;
        self.cloud_states = snapshot.states;
        self.sync_cloud_stopped(cx);
        if changed {
            self.invalidate_chrome(cx);
            cx.notify();
        }
    }

    /// The cloud mark for a session row: a cloud glyph, the codespace state's dot
    /// and, in roomy rows, its name.
    pub(super) fn cloud_badge(&self, id: i64, compact: bool) -> Option<AnyElement> {
        let mark = self.cloud_marks.get(&id)?;
        let mut indicator = indicator(self.cloud_states.get(&mark.key()).copied());
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
                .aria_label(format!(
                    "{} {}: {}",
                    mark.kind.title(),
                    mark.codespace,
                    indicator.label
                ))
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

    /// A local PTY running `ssh -t` into the container of sandbox session `id`. `None`, with
    /// the reason in the window's error line, when the sandbox connection cannot be found.
    pub(super) fn spawn_sandbox_terminal(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        let command = match self.cloud.sandbox_terminal(id) {
            Ok(command) => command,
            Err(error) => {
                self.error = Some(format!("Could not open a terminal in the sandbox: {error}"));
                cx.notify();
                return None;
            }
        };
        let cwd = crate::terminal::home_cwd();
        // The ssh alias runs `sbx` as its ProxyCommand, which a Finder-launched app cannot
        // find on launchd's bare PATH.
        let env = vec![(
            "PATH".to_string(),
            OsString::from(monocode_core::scm::terminal_search_path()),
        )];
        let terminal = cx.new(|cx| {
            Terminal::launch(
                std::path::Path::new(&cwd),
                ssh_program(),
                sandbox_ssh_args(&command),
                env,
                Vec::new(),
                cx,
            )
        });
        self.observations
            .push(cx.subscribe(&terminal, |_, _, _: &TerminalEvent, cx| cx.notify()));
        Some(terminal)
    }

    /// The terminal that belongs to cloud session `id`: `gh codespace ssh` for a codespace,
    /// `ssh -t` into the container for a sandbox. A sandbox whose terminal cannot be opened
    /// falls back to a local shell at home, with the reason shown.
    pub(super) fn spawn_cloud_terminal(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        let mark = self.cloud_marks.get(&id)?.clone();
        match mark.kind {
            CloudBackendKind::Codespaces => {
                Some(self.spawn_codespace_terminal(&mark.codespace, cx))
            }
            CloudBackendKind::DockerSandbox => self.spawn_sandbox_terminal(id, cx),
        }
    }

    /// Terminal for a cloud session: a local PTY running `gh codespace ssh`, or `ssh` into
    /// the container for a sandbox.
    pub(super) fn new_cloud_terminal(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = crate::terminal::home_cwd();
        let project = self.workspace.project.clone();
        // Resolve the terminal first: when it can not be opened (the reason is on screen)
        // no dock tab is added and nothing is spawned.
        let Some(terminal) = self.spawn_cloud_terminal(id, cx) else {
            return;
        };
        let was_open = self
            .workspace
            .docks
            .get(&project)
            .is_some_and(|d| d.open && !d.terminals.is_empty());
        self.ensure_terminals(&project, cx);
        let viewport = self.viewport();
        let dock = self.workspace.docks.entry(project.clone()).or_default();
        dock.size = dock.side.clamp(dock.size, viewport);
        let added = dock.add(cwd);
        self.dock_tab_scroll.scroll_to_item(added);
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

    /// What the File and Git panels are bound to: the workspace of the cloud session in view
    /// (default), its local checkout when the user switched the panels to Local, or the local
    /// project. Pure reads: it never touches a workspace.
    pub(super) fn panel_root(&self, cx: &App) -> crate::panel_root::PanelRoot {
        use crate::panel_root::{ActiveSession, PanelMode, resolve};
        let id = self
            .workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane));
        let mode = id
            .and_then(|id| self.panel_modes.get(&id).copied())
            .unwrap_or(PanelMode::Cloud);
        let mark = id.and_then(|id| self.cloud_marks.get(&id));
        let live = id.and_then(|id| self.sessions.get(&id)).map(|v| v.read(cx));
        // A cloud session without a live view still gets its cloud root, from its summary.
        let facts = match (id, live) {
            (_, Some(view)) => Some((
                view.summary.cwd.as_str(),
                view.settings.worktree_cwd.as_deref(),
            )),
            (Some(id), None) if mark.is_some() => self
                .session_summaries
                .get(&id)
                .map(|summary| (summary.cwd.as_str(), None)),
            _ => None,
        };
        let active = facts.map(|(cwd, worktree_cwd)| ActiveSession {
            cwd,
            worktree_cwd,
            cloud: mark.map(|mark| (mark.cloud(), mark.local_project.as_str())),
        });
        resolve(active, mode, self.mono_active(cx), self.working_copy())
    }

    /// Switches the panels of the session in view between its cloud workspace and its local
    /// checkout. The choice is per session and lives in memory.
    pub(super) fn set_panel_mode(
        &mut self,
        mode: crate::panel_root::PanelMode,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane))
        else {
            return;
        };
        if self.panel_modes.insert(id, mode).unwrap_or_default() != mode {
            self.sync_git(cx);
            cx.notify();
        }
    }

    /// Whether the app has a live connection to the workspace of the session in view. A
    /// session without a live view is Not connected. Never wakes anything.
    pub(super) fn panel_link(&self, cx: &App) -> crate::panel_root::Link {
        let id = self
            .workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane));
        id.and_then(|id| self.sessions.get(&id))
            .map_or(crate::panel_root::Link::NotConnected, |view| {
                view.read(cx).panel_link()
            })
    }

    /// Whether the link of the session in view is a fact: a live conversation says the workspace
    /// is up, waking or down. A session without one (it runs in a terminal) or one that has not
    /// finished its first refresh knows nothing, and its panels read as they always did.
    fn panel_link_known(&self, cx: &App) -> bool {
        self.workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane))
            .and_then(|id| self.sessions.get(&id))
            .is_some_and(|view| view.read(cx).panel_link_known())
    }

    /// The panels show remembered data of a workspace the app knows is down.
    pub(super) fn panel_offline(&self, root: &crate::panel_root::PanelRoot, cx: &App) -> bool {
        crate::panel_root::is_offline(root, self.panel_link(cx)) && self.panel_link_known(cx)
    }

    /// Every Host environment of a cloud session whose link is known to be down, for the git
    /// gate: a Review tab of workspace B must be refused while the panels show A or Local.
    fn offline_environments(&self, cx: &App) -> Vec<String> {
        let mut envs = Vec::new();
        for id in self.cloud_marks.keys() {
            let Some(view) = self.sessions.get(id).map(|view| view.read(cx)) else {
                continue;
            };
            if !view.panel_link_known() || view.panel_link() == crate::panel_root::Link::Connected {
                continue;
            }
            let cwds = self
                .session_summaries
                .get(id)
                .map(|s| s.cwd.as_str())
                .into_iter()
                .chain(view.settings.worktree_cwd.as_deref());
            for cwd in cwds {
                if let Some(path) = monocode_core::remote::parse_remote_path(cwd)
                    && !envs.contains(&path.environment_id)
                {
                    envs.push(path.environment_id);
                }
            }
        }
        envs
    }

    /// Whether the session in view has a live conversation for the Connect action to reconnect.
    fn panel_can_connect(&self) -> bool {
        self.workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane))
            .is_some_and(|id| self.sessions.contains_key(&id))
    }

    /// The Connect action of the connection strip. It is the Reconnect button of the session in
    /// view, a user action: the only way the panels ever wake a workspace.
    pub(super) fn connect_panel_workspace(&self, cx: &mut Context<Self>) {
        let id = self
            .workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane));
        if let Some(view) = id.and_then(|id| self.sessions.get(&id)).cloned() {
            view.update(cx, |view, cx| {
                view.reconnect_cloud(cx);
            });
        }
    }

    /// The Host environments where a cloud session has a turn running. The app-side half of the
    /// "not while the agent works" rule, see `git_gate::turn_running`. Every cloud session counts,
    /// not just the one in view: a Review of another workspace asks the same gate.
    fn running_environments(&self, cx: &App) -> Vec<String> {
        let mut envs = Vec::new();
        for id in self.cloud_marks.keys() {
            if !self.session_summaries.contains_key(id) || !self.session_metadata(*id, cx).busy {
                continue;
            }
            let cwds = self
                .session_summaries
                .get(id)
                .map(|s| s.cwd.as_str())
                .into_iter()
                .chain(
                    self.sessions
                        .get(id)
                        .and_then(|v| v.read(cx).settings.worktree_cwd.as_deref()),
                );
            for cwd in cwds {
                if let Some(path) = monocode_core::remote::parse_remote_path(cwd)
                    && !envs.contains(&path.environment_id)
                {
                    envs.push(path.environment_id);
                }
            }
        }
        envs
    }

    /// Opens the codespace of the panels in the browser (VS Code for the web). The URL is read
    /// on a worker; a failure lands in the file-error notice.
    pub(super) fn open_panel_in_browser(&mut self, cx: &mut Context<Self>) {
        let Some(cloud) = self.panel_root(cx).cloud().cloned() else {
            return;
        };
        let service = self.cloud.clone();
        let work = cx
            .background_executor()
            .spawn(async move { service.workspace_web_url(&cloud) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(url) => cx.open_url(&url),
                Err(error) => {
                    this.notification_state
                        .file_error
                        .update(cx, |notice, cx| notice.set_message(Some(error), cx));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Runs every frame, from the shell's render: binds the panels to the resolved root, draws the
    /// cloud banner, switch and connection strip, and tells the git gate what the panels show.
    /// Nothing here may wake a workspace: it only reads cached state.
    pub(super) fn sync_cloud_panel(&self, cx: &mut Context<Self>) {
        let root = self.panel_root(cx);
        let link = self.panel_link(cx);
        let known = self.panel_link_known(cx);
        let offline = self.panel_offline(&root, cx);
        // A cloud session's root can change without the session changing (the Local switch, a
        // pending workspace that became real). Offline: the panels keep the last data and read
        // nothing; back online: a full reload.
        let project = root.project().to_owned();
        if root.cloud().is_some() {
            self.git
                .update(cx, |git, cx| git.bind(project, offline, cx));
        } else {
            self.git
                .update(cx, |git, cx| git.set_link_offline(false, cx));
        }
        self.git.update(cx, |git, cx| {
            git.set_starting(offline && link == crate::panel_root::Link::Starting, cx)
        });
        let running = self.running_environments(cx);
        let (age, recovering) = {
            let git = self.git.read(cx);
            (git.cached_age(), git.stale() && !git.offline())
        };
        let chrome = crate::panel_root::Chrome::of(&root, link, self.panel_can_connect())
            .map(|chrome| chrome.with_state(known, recovering).with_age(age));
        self.sync_codespace_pane(cx);
        let mut offline_envs = self.offline_environments(cx);
        if chrome.as_ref().is_some_and(|chrome| chrome.offline)
            && let Some(path) = monocode_core::remote::parse_remote_path(root.project())
            && !offline_envs.contains(&path.environment_id)
        {
            offline_envs.push(path.environment_id);
        }
        let flipped = crate::git_gate::set(crate::git_gate::Gate::of(&root, running, offline_envs));
        if let Some(panel) = &self.right_panel {
            panel.update(cx, |panel, cx| {
                panel.set_cloud_chrome(chrome, cx);
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

    /// The workspace of the active cloud session, when one is in view.
    #[cfg(test)]
    pub(super) fn active_cloud(&self) -> Option<CloudRef> {
        let id = self.workspace.active?;
        self.cloud_marks.get(&id).map(CloudMark::cloud)
    }

    /// What a worker thread needs to answer "is this instance busy" for `cloud.stop` /
    /// `cloud.delete`: the workspaces an archive is working on right now and the cloud sessions
    /// whose live view is working or waiting (the stored flags lag behind both).
    pub(crate) fn cloud_busy_snapshot(&self, cx: &App) -> crate::cloud_actions::LiveBusy {
        crate::cloud_actions::LiveBusy {
            workspaces: self.cloud_workspaces_busy.clone(),
            sessions: self
                .session_summaries
                .iter()
                .filter(|(_, summary)| summary.cwd.starts_with("remote://"))
                .map(|(id, _)| *id)
                .filter(|id| self.session_metadata(*id, cx).busy)
                .collect(),
        }
    }

    /// Whether a cloud archive is converting session `id` right now.
    pub(crate) fn cloud_archive_in_progress(&self, id: i64) -> bool {
        self.cloud_busy.contains(&id)
    }

    /// Whether an archive is already working on the workspace of session `id`.
    pub(super) fn workspace_busy_of(&self, id: i64) -> bool {
        self.cloud_marks
            .get(&id)
            .is_some_and(|mark| self.cloud_workspaces_busy.contains(&mark.key()))
    }

    /// Non-archived sessions outside `selected` that still use `target`.
    pub(super) fn other_users_of(&self, target: &CloudRef, selected: &[i64]) -> usize {
        self.cloud_marks
            .iter()
            .filter(|(other, m)| {
                m.cloud() == *target
                    && !selected.contains(other)
                    && self
                        .session_summaries
                        .get(other)
                        .is_some_and(|s| !s.archived)
            })
            .count()
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
        if ids.iter().any(|id| self.workspace_busy_of(*id)) {
            self.error = Some(WORKSPACE_BUSY.into());
            cx.notify();
            return;
        }
        let groups = group_by_codespace(&ids, &self.cloud_marks, |target, selected| {
            self.other_users_of(target, selected)
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
            files_only: Vec::new(),
            deferred: Vec::new(),
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
        self.run_workspace_action(CloudRef::codespace(codespace), choice, window, cx);
    }

    /// Stops or deletes the workspace `target` without archiving a session (session menu).
    /// Deleting saves every session still in it and asks first, as the archive dialog does.
    pub(super) fn run_workspace_action(
        &mut self,
        target: CloudRef,
        choice: ArchiveChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_archive.is_some() {
            return;
        }
        if self.cloud_workspaces_busy.contains(&target.key()) {
            self.error = Some(WORKSPACE_BUSY.into());
            cx.notify();
            return;
        }
        let total = self
            .cloud_marks
            .values()
            .filter(|m| m.cloud() == target)
            .count();
        self.close_preview(cx);
        self.cloud_archive = Some(ArchiveDialog {
            groups: vec![CodespaceGroup {
                codespace: target.name.clone(),
                kind: target.kind,
                ids: vec![],
                others: 0,
                total,
            }],
            title: String::new(),
            phase: Phase::Choose,
            files_only: Vec::new(),
            deferred: Vec::new(),
            focus: cx.focus_handle(),
        });
        if choice == ArchiveChoice::Delete {
            let items = self.cloud.unsaved_for_cloud(&target);
            if let Some(dialog) = self.cloud_archive.as_mut() {
                dialog.phase = Phase::ConfirmDelete { items };
                window.focus(&dialog.focus, cx);
            }
            cx.notify();
            return;
        }
        if choice == ArchiveChoice::Stop && self.workspace_has_running_turn(&target) {
            if let Some(dialog) = self.cloud_archive.as_mut() {
                dialog.phase = Phase::ConfirmStop;
                window.focus(&dialog.focus, cx);
            }
            cx.notify();
            return;
        }
        self.run_cloud_archive(choice, false, Vec::new(), window, cx);
    }

    /// Whether any session living in `target` has an agent turn running.
    pub(super) fn workspace_has_running_turn(&self, target: &CloudRef) -> bool {
        self.cloud_marks
            .iter()
            .filter(|(_, m)| m.cloud() == *target)
            .any(|(id, _)| {
                self.remote_marks
                    .get(id)
                    .is_some_and(|(running, _)| *running)
            })
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
        let files_only = dialog.files_only.clone();
        window.focus(&dialog.focus, cx);
        self.cloud_busy.extend(ids.iter().copied());
        let keys: Vec<String> = groups.iter().map(|g| g.cloud().key()).collect();
        self.cloud_workspaces_busy.extend(keys.iter().cloned());
        // Delete saves every session of the codespace, so all of them stop taking messages.
        let mut locked_ids = ids.clone();
        if choice == ArchiveChoice::Delete {
            for group in &groups {
                let target = group.cloud();
                locked_ids.extend(
                    self.cloud_marks
                        .iter()
                        .filter(|(_, m)| m.cloud() == target)
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
                &files_only,
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
                for key in &keys {
                    this.cloud_workspaces_busy.remove(key);
                }
                this.finish_cloud_archive(choice, ids, outcomes, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    /// The user agreed to save only the code of session `id`: run the archive again.
    pub(super) fn confirm_files_only(
        &mut self,
        choice: ArchiveChoice,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.cloud_archive.as_mut()
            && !dialog.files_only.contains(&id)
        {
            dialog.files_only.push(id);
        }
        self.run_cloud_archive(choice, true, Vec::new(), window, cx);
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

    /// Applies what the archive workers did to the window (adopt the local copies, archive or
    /// forget the sessions) without touching the dialog, so an app call can use it while the
    /// user has a dialog of their own open.
    pub(super) fn settle_cloud_archive(
        &mut self,
        choice: ArchiveChoice,
        ids: Vec<i64>,
        outcomes: Vec<GroupOutcome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Settled {
        self.cloud_busy.retain(|id| !ids.contains(id));
        // Conversion warnings are notices; a partial codespace failure stays an error.
        let mut notes = Vec::new();
        let mut warnings = Vec::new();
        let mut failed: Vec<(CodespaceGroup, String)> = Vec::new();
        let mut review: Vec<(CodespaceGroup, Vec<String>)> = Vec::new();
        let mut unreachable: Vec<Unreachable> = Vec::new();
        for outcome in outcomes {
            let GroupOutcome {
                mut group,
                converted,
                result,
                risky,
                unreachable: unreached,
            } = outcome;
            if let Some((id, reason)) = unreached {
                let title = self.session_metadata(id, cx).display_title();
                unreachable.push(Unreachable {
                    group: group.clone(),
                    id,
                    title,
                    reason,
                });
            }
            for saved in converted {
                if !saved.history_saved {
                    warnings.push(format!(
                        "The conversation of {} could not be saved; only the code was.",
                        quoted_title(&saved.title)
                    ));
                }
                warnings.extend(saved.warnings.iter().cloned());
                self.adopt_converted(saved, window, cx);
            }
            group.ids.retain(|id| self.is_cloud(*id));
            match result {
                Ok((then, warning)) => {
                    notes.extend(warning);
                    match then {
                        Then::Archive => {
                            if choice == ArchiveChoice::Stop {
                                self.cloud_states
                                    .insert(group.cloud().key(), CloudState::Stopping);
                            }
                            for id in group.ids {
                                self.toggle_archive_now(id, cx);
                            }
                        }
                        Then::Forget => {
                            self.cloud_gone.insert(group.cloud().key());
                            self.forget_codespace_sessions(&group.cloud(), window, cx)
                        }
                    }
                }
                Err(error) if error == CANCELLED => {}
                Err(error) if error == NEEDS_FILES_ONLY => {}
                Err(error) if error == NEEDS_REVIEW => review.push((group, risky)),
                Err(error) => failed.push((group, error)),
            }
        }
        if !notes.is_empty() {
            self.error = Some(notes.join(" "));
        }
        if !warnings.is_empty() {
            self.notice = Some(warnings.join(" "));
        }
        Settled {
            failed,
            review,
            unreachable,
        }
    }

    pub(super) fn finish_cloud_archive(
        &mut self,
        choice: ArchiveChoice,
        ids: Vec<i64>,
        outcomes: Vec<GroupOutcome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Settled {
            failed,
            review,
            mut unreachable,
        } = self.settle_cloud_archive(choice, ids, outcomes, window, cx);
        // Paused groups wait while another group fails: "anyway" must not reach them.
        if let Some(dialog) = self.cloud_archive.as_mut() {
            if !failed.is_empty() {
                dialog.deferred.append(&mut unreachable);
            } else if unreachable.is_empty() && review.is_empty() {
                unreachable = std::mem::take(&mut dialog.deferred);
            }
        }
        if failed.is_empty() && review.is_empty() && unreachable.is_empty() {
            if self.cloud_archive.take().is_some() {
                self.focus_active(window, cx);
            }
        } else if let Some(dialog) = self.cloud_archive.as_mut() {
            if failed.is_empty() && !unreachable.is_empty() {
                let first = unreachable[0].clone();
                dialog.groups = unreachable
                    .into_iter()
                    .map(|u| u.group)
                    .chain(review.into_iter().map(|(group, _)| group))
                    .collect();
                dialog.phase = Phase::FilesOnly {
                    choice,
                    id: first.id,
                    title: first.title,
                    reason: first.reason,
                };
            } else if failed.is_empty() {
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
        let mark = self.cloud_marks.remove(&cloud_id);
        self.remote_marks.remove(&cloud_id);
        self.forget_session(cloud_id, cx);
        let id = saved.local_id;
        // Origins are codespace names only: a session saved from a sandbox has no codespace
        // to stop or delete from its menu.
        if mark.is_none_or(|mark| mark.kind == CloudBackendKind::Codespaces) {
            self.cloud_origins.insert(id, saved.codespace.clone());
        }
        self.workspace.add_project(&saved.summary.cwd);
        self.session_summaries.insert(id, saved.summary);
        self.order.retain(|s| *s != id);
        self.order.insert(0, id);
    }

    /// Drops every session of a deleted codespace or sandbox from the window.
    pub(super) fn forget_codespace_sessions(
        &mut self,
        target: &CloudRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gone: Vec<i64> = self
            .cloud_marks
            .iter()
            .filter(|(_, mark)| mark.cloud() == *target)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.forget_cloud_row(id, window, cx);
        }
        self.cloud_states.remove(&target.key());
    }

    /// Drops one cloud session from the window; its record is gone from the store already.
    pub(super) fn forget_cloud_row(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.tab_of(id).is_some() {
            self.close_pane(id, window, cx);
        }
        self.cloud_marks.remove(&id);
        self.remote_marks.remove(&id);
        self.forget_session(id, cx);
    }

    /// Asks before "Remove from Monoptah" of a stopped sandbox's session `id`.
    pub(super) fn confirm_remove_cloud_session(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mark) = self.cloud_marks.get(&id).cloned() else {
            return;
        };
        if self.cloud_archive.is_some() || self.cloud_busy.contains(&id) {
            return;
        }
        if self.cloud_workspaces_busy.contains(&mark.key()) {
            self.error = Some(WORKSPACE_BUSY.into());
            cx.notify();
            return;
        }
        let title = self.session_metadata(id, cx).display_title();
        self.close_preview(cx);
        let dialog = ArchiveDialog {
            groups: vec![CodespaceGroup {
                codespace: mark.codespace.clone(),
                kind: mark.kind,
                ids: vec![],
                others: 0,
                total: 1,
            }],
            title: String::new(),
            phase: Phase::ConfirmRemove {
                id,
                title,
                name: mark.codespace,
            },
            files_only: Vec::new(),
            deferred: Vec::new(),
            focus: cx.focus_handle(),
        };
        window.focus(&dialog.focus, cx);
        self.cloud_archive = Some(dialog);
        cx.notify();
    }

    /// "Remove from Monoptah" for a sandbox session whose sandbox is stopped: forgets the
    /// local record only (the sandbox keeps existing) and shows the store's warning. Never
    /// contacts the sandbox, so it can not resume it.
    pub(super) fn remove_cloud_session_locally(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_cloud(id) || self.cloud_busy.contains(&id) {
            return;
        }
        self.cloud_busy.insert(id);
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store).and_then(|store| store.remove_cloud_session_locally(id))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.cloud_busy.remove(&id);
                match result {
                    Ok(warning) => {
                        this.forget_cloud_row(id, window, cx);
                        this.notice = Some(warning);
                    }
                    Err(error) => {
                        this.error = Some(format!("Could not remove the session: {error}"));
                    }
                }
                this.invalidate_chrome(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn cloud_archive_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.cloud_archive.as_ref()?;
        let default = default_choice(dialog.others());
        let words = words(&dialog.groups);
        let width = 560f32.min(window.viewport_size().width.as_f32() - 24.);
        let weak = cx.weak_entity();
        let button =
            |id: &'static str, label: String, choice: ArchiveChoice, cx: &mut Context<Self>| {
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
                "Archive cloud session?".to_string(),
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(text(
                        "cloud-archive-summary",
                        format!(
                            "{} {} Stopping or deleting it first saves an archived local copy \
                             with the conversation, evidence and code (branches, the stash and \
                             uncommitted changes; ignored files are listed but not saved). {}",
                            archive_summary(&dialog.title, &dialog.groups),
                            words.cost(),
                            delete_note(&dialog.groups),
                        ),
                        false,
                    ))
                    .children(others_note(dialog.others(), words.noun).map(|note| {
                        text(
                            "cloud-archive-others",
                            format!(
                                "{note} Deleting the {} removes those sessions from Monoptah too.",
                                words.noun
                            ),
                            true,
                        )
                    })),
                footer
                    .child(cancel)
                    .child(button(
                        "cloud-archive-keep",
                        "Keep running".into(),
                        ArchiveChoice::Keep,
                        cx,
                    ))
                    .child(button(
                        "cloud-archive-delete",
                        format!("Delete {}", words.noun),
                        ArchiveChoice::Delete,
                        cx,
                    ))
                    .child(button(
                        "cloud-archive-stop",
                        format!("Stop {}", words.noun),
                        ArchiveChoice::Stop,
                        cx,
                    )),
            ),
            Phase::Working { progress, .. } => (
                "Saving a local copy".to_string(),
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
            Phase::ConfirmStop => {
                let name = dialog
                    .groups
                    .first()
                    .map(|g| g.codespace.clone())
                    .unwrap_or_default();
                (
                    format!("Stop {}?", words.noun),
                    div().child(text(
                        "cloud-archive-confirm-stop-body",
                        format!("{name} has a running turn. Stopping interrupts it."),
                        false,
                    )),
                    footer.child(cancel).child(
                        crate::modal::danger_button("cloud-archive-confirm-stop", "Stop")
                            .debug_selector(|| "cloud-archive-confirm-stop".into())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
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
            Phase::ConfirmDelete { items } => {
                let seen = items.clone();
                let name = dialog
                    .groups
                    .first()
                    .map(|g| g.codespace.clone())
                    .unwrap_or_default();
                let mut lines = vec![delete_confirm_line(&words, &name)];
                let stopped = dialog.groups.first().is_some_and(|g| {
                    self.cloud_states.get(&g.cloud().key()) == Some(&CloudState::Stopped)
                });
                lines.extend(delete_start_note(&words, stopped));
                lines.extend(items.iter().cloned());
                (
                    format!("Delete {}?", words.noun),
                    div().child(text("cloud-archive-confirm", lines.join(" "), false)),
                    footer.child(cancel).child(
                        crate::modal::danger_button(
                            "cloud-archive-confirm-delete",
                            format!("Delete {}", words.noun),
                        )
                        .debug_selector(|| "cloud-archive-confirm-delete".into())
                        .cursor_pointer()
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run_cloud_archive(
                                    ArchiveChoice::Delete,
                                    true,
                                    seen.clone(),
                                    window,
                                    cx,
                                )
                            },
                        )),
                    ),
                )
            }
            Phase::ConfirmRemove { id, title, name } => {
                let id = *id;
                (
                    "Remove from Monoptah?".to_string(),
                    div().child(text(
                        "cloud-archive-remove",
                        remove_confirm_body(title, name),
                        false,
                    )),
                    footer.child(cancel).child(
                        crate::modal::danger_button("cloud-archive-remove-go", "Remove")
                            .debug_selector(|| "cloud-archive-remove-go".into())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.cloud_archive = None;
                                this.focus_active(window, cx);
                                this.remove_cloud_session_locally(id, window, cx)
                            })),
                    ),
                )
            }
            Phase::FilesOnly {
                choice,
                id,
                title,
                reason,
            } => {
                let (choice, id) = (*choice, *id);
                (
                    "Archive files only?".to_string(),
                    div().child(text(
                        "cloud-archive-files-only",
                        files_only_body(title, reason),
                        false,
                    )),
                    footer
                        .child(cancel)
                        .child(
                            crate::modal::ghost_button("cloud-archive-files-retry", "Retry")
                                .debug_selector(|| "cloud-archive-files-retry".into())
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_cloud_archive(choice, true, Vec::new(), window, cx)
                                })),
                        )
                        .child(
                            crate::modal::primary_button(
                                "cloud-archive-files-only-go",
                                "Archive files only",
                            )
                            .debug_selector(|| "cloud-archive-files-only-go".into())
                            .cursor_pointer()
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.confirm_files_only(choice, id, window, cx)
                                },
                            )),
                        ),
                )
            }
            Phase::Review { items } => {
                let seen = items.clone();
                (
                    "Some files are not saved".to_string(),
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(text("cloud-archive-risk", items.join(" "), false))
                        .child(text("cloud-archive-risk-note", review_note(&words), true)),
                    footer
                        .child(
                            crate::modal::ghost_button("cloud-archive-review-keep", "Keep running")
                                .debug_selector(|| "cloud-archive-review-keep".into())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_cloud_archive(window, cx)
                                })),
                        )
                        .child(
                            crate::modal::danger_button(
                                "cloud-archive-delete-anyway",
                                "Delete anyway",
                            )
                            .debug_selector(|| "cloud-archive-delete-anyway".into())
                            .cursor_pointer()
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    // Copies any session that appeared meanwhile, then asks again
                                    // if that found more to lose.
                                    this.run_cloud_archive(
                                        ArchiveChoice::Delete,
                                        true,
                                        seen.clone(),
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        )
                        .child(
                            crate::modal::primary_button(
                                "cloud-archive-stop-instead",
                                "Stop instead",
                            )
                            .debug_selector(|| "cloud-archive-stop-instead".into())
                            .cursor_pointer()
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.run_cloud_archive(
                                        ArchiveChoice::Stop,
                                        false,
                                        Vec::new(),
                                        window,
                                        cx,
                                    )
                                },
                            )),
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
                    "Could not finish".to_string(),
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(text("cloud-archive-error", error.clone(), false))
                        .child(text("cloud-archive-warning", failed_note(&words), true)),
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
