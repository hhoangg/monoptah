//! What the Cloud workspaces dialog says and decides: the words for a state, an age, a cost
//! line, the summary and the notices about a backend that could not be read. It holds no
//! widgets and runs no CLI, so tests read it directly.
use crate::cloud_checks::gh_state;
use crate::cloud_mark::{Tone, backend_name};
use monocode_core::cloud_sessions::{
    CloudBackendKind, CloudRef,
    workspaces::{DeletePreview, WorkspaceOverview, WorkspaceRow, WorkspaceState},
};
use monocode_core::sandboxes::Prerequisites;

pub(super) const ESTIMATE_NOTE: &str = "Costs are estimates, never a bill. Docker rates come from Docker's usage page (per runtime second) and Codespaces use $0.09 per core-hour. Neither CLI reports uptime, so a workspace that is up is counted from its creation (\"at most\"). Free allowances, storage and Docker's credit are not modelled; check GitHub billing and Docker usage for the real numbers.";
pub(super) const NO_WORKSPACES: &str = "No cloud workspaces found for this project.";
pub(super) const NOT_ON_GITHUB: &str =
    "This project's origin is not on GitHub, so no codespaces are listed.";

/// Which backend a row belongs to, as the cloud badge words it.
pub(super) fn backend(row: &WorkspaceRow) -> &'static str {
    backend_name(row.cloud.kind)
}

/// `45s`, `12m`, `3h 12m`, `2d 4h`.
pub(super) fn age_text(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d {}h", secs / 86_400, secs % 86_400 / 3600),
    }
}

/// `$0.56`, `<$0.01`, `$123`.
pub(super) fn money(usd: f64) -> String {
    if usd < 0.005 {
        "<$0.01".into()
    } else if usd >= 100. {
        format!("${usd:.0}")
    } else {
        format!("${usd:.2}")
    }
}

/// The state dot's tone: running is the one that costs, so it is the loud one.
pub(super) fn state_tone(state: WorkspaceState) -> Tone {
    match state {
        WorkspaceState::Running => Tone::Good,
        WorkspaceState::Starting | WorkspaceState::Stopping | WorkspaceState::Hibernating => {
            Tone::Busy
        }
        WorkspaceState::Failed => Tone::Bad,
        WorkspaceState::Stopped | WorkspaceState::Unknown => Tone::Idle,
    }
}

/// `Created 3h 12m ago · Last used 20m ago`, whatever is known.
pub(super) fn age_line(row: &WorkspaceRow, now_ms: u64) -> Option<String> {
    let created = row
        .age_secs(now_ms)
        .map(|secs| format!("Created {} ago", age_text(secs)));
    let used = row.last_used_ms.map(|ms| {
        format!(
            "Last used {} ago",
            age_text(now_ms.saturating_sub(ms) / 1000)
        )
    });
    let parts: Vec<String> = created.into_iter().chain(used).collect();
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// `fix/login · Medium · 4 vCPU / 8g · Docker account me`: branch, size and the account of a
/// sandbox, whatever is known.
pub(super) fn detail_line(row: &WorkspaceRow) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(branch) = &row.branch {
        parts.push(branch);
    }
    parts.push(&row.size);
    if let Some(account) = &row.account_label {
        parts.push(account);
    }
    parts.join(" \u{b7} ")
}

/// What GitHub last said a codespace holds that git does not have, so the row warns before the
/// user opens Delete. `None` when it reported nothing to warn about.
pub(super) fn git_line(row: &WorkspaceRow) -> Option<String> {
    let git = row.git?;
    let mut parts = Vec::new();
    if git.uncommitted {
        parts.push("uncommitted changes");
    }
    if git.unpushed {
        parts.push("unpushed commits");
    }
    (!parts.is_empty()).then(|| format!("Has {} (as GitHub last reported)", parts.join(" and ")))
}

/// How a row relates to this app, for its colour and its words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Use {
    /// Live sessions use it.
    Sessions,
    /// This app made it and nothing uses it any more: the one the overview exists to find.
    Orphan,
    /// This app's store does not know it: another app, another computer or a person made it.
    Foreign,
}

/// Who uses the workspace, and whether it is this app's at all.
pub(super) fn uses_line(row: &WorkspaceRow) -> (String, Use) {
    if !row.ours {
        let used = if row.sessions.is_empty() {
            String::new()
        } else {
            format!(" Sessions here use it: {}.", titles(row))
        };
        return (
            format!(
                "Not created by this app \u{2014} another app, another computer or a person made it and may be using it.{used}"
            ),
            Use::Foreign,
        );
    }
    if row.sessions.is_empty() {
        return (
            "No Monoptah session uses it \u{2014} nothing will stop it for you".into(),
            Use::Orphan,
        );
    }
    (format!("Used by {}", titles(row)), Use::Sessions)
}

fn titles(row: &WorkspaceRow) -> String {
    let name = |s: &monocode_core::cloud_sessions::workspaces::UsingSession| {
        let title = s.title.trim();
        let title = if title.is_empty() { "Untitled" } else { title };
        if s.busy {
            format!("{title} (working)")
        } else {
            title.to_owned()
        }
    };
    let shown: Vec<String> = row.sessions.iter().take(3).map(name).collect();
    let more = row.sessions.len().saturating_sub(3);
    if more > 0 {
        format!("{} and {more} more", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

/// The estimated cost line of a row. Always says it is an estimate.
pub(super) fn cost_line(row: &WorkspaceRow) -> String {
    let cost = &row.cost;
    let rate = |prefix: &str| {
        cost.rate_per_hour
            .map(|r| format!("{prefix}\u{2248} {}/h", money(r)))
    };
    let line = match row.state {
        state if state.accruing() => {
            let so_far = match (cost.total, cost.runtime_secs) {
                (Some(total), Some(secs)) => format!(
                    "\u{2248} {} so far (at most {} running)",
                    money(total),
                    age_text(secs)
                ),
                (None, Some(secs)) => format!("Running for at most {}", age_text(secs)),
                _ => "Running; the start time is not known".into(),
            };
            match rate(" \u{b7} ") {
                Some(rate) => format!("{so_far}{rate} while it runs"),
                None => format!("{so_far} \u{b7} this size has no known rate"),
            }
        }
        WorkspaceState::Stopped => {
            let mut text = "Not running, so no compute cost".to_owned();
            if let Some(rate) = rate(" \u{b7} ") {
                text.push_str(&format!("{rate} when it runs"));
            }
            if row.cloud.kind == CloudBackendKind::Codespaces {
                text.push_str(" \u{b7} storage may still be billed");
            }
            text
        }
        _ => "State unclear: it may still count against your quota".into(),
    };
    format!("Est. {line}")
}

/// Counts for the header line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Summary {
    pub total: usize,
    /// Rows that are, or may be, drawing compute.
    pub running: usize,
    /// Made by this app and used by no session.
    pub orphans: usize,
    /// Not made by this app (see [`WorkspaceRow::ours`]).
    pub foreign: usize,
    /// Estimated USD per hour of the running rows whose size has a rate.
    pub burn: f64,
}

pub(super) fn summary(view: &WorkspaceOverview) -> Summary {
    let mut sum = Summary {
        total: view.rows.len(),
        ..Summary::default()
    };
    for row in &view.rows {
        sum.orphans += usize::from(row.orphan());
        sum.foreign += usize::from(!row.ours);
        if row.state.accruing() {
            sum.running += 1;
            sum.burn += row.cost.rate_per_hour.unwrap_or(0.);
        }
    }
    sum
}

pub(super) fn summary_line(sum: &Summary) -> String {
    if sum.total == 0 {
        return NO_WORKSPACES.into();
    }
    let mut line = format!(
        "{} workspace{} \u{b7} {} running",
        sum.total,
        if sum.total == 1 { "" } else { "s" },
        sum.running
    );
    if sum.running > 0 && sum.burn > 0. {
        line.push_str(&format!(
            " \u{b7} est. {}/h while they run",
            money(sum.burn)
        ));
    }
    if sum.orphans > 0 {
        line.push_str(&format!(" \u{b7} {} with no session", sum.orphans));
    }
    if sum.foreign > 0 {
        line.push_str(&format!(" \u{b7} {} not created by this app", sum.foreign));
    }
    line
}

/// A line about a backend that could not be read, with the command that fixes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Notice {
    pub id: &'static str,
    pub text: String,
    pub fix: Option<String>,
    /// Something is wrong (shown as an alert); otherwise it only explains why nothing is listed.
    pub problem: bool,
}

/// Why a backend lists nothing: the prerequisite diagnosis and its fix hints, as the setup
/// dialogs word them. Empty when both backends were read.
pub(super) fn notices(view: &WorkspaceOverview) -> Vec<Notice> {
    let mut out = Vec::new();
    match &view.gh {
        None => out.push(Notice {
            id: "cloud-workspaces-no-repo",
            text: NOT_ON_GITHUB.into(),
            fix: None,
            problem: false,
        }),
        Some(result) => {
            let gh = gh_state(result.clone());
            if !gh.ready() {
                out.push(Notice {
                    id: "cloud-workspaces-gh",
                    text: format!("Codespaces are not listed. {}.", gh.line()),
                    fix: gh.fix().map(str::to_owned),
                    problem: true,
                });
            }
        }
    }
    if let Some(error) = &view.codespaces_error {
        out.push(Notice {
            id: "cloud-workspaces-gh-list",
            text: format!("Could not list codespaces: {error}"),
            fix: None,
            problem: true,
        });
    }
    for (index, found) in view.sandbox_problems.iter().enumerate() {
        let whose = found
            .account
            .as_deref()
            .map(|account| format!(" for {account}"))
            .unwrap_or_default();
        out.push(Notice {
            id: if index == 0 {
                "cloud-workspaces-sbx"
            } else {
                "cloud-workspaces-sbx-more"
            },
            text: format!(
                "Docker sandboxes are not listed{whose}. {}",
                found.problem.message()
            ),
            fix: found.problem.fix_command().map(str::to_owned),
            problem: !matches!(found.problem, Prerequisites::SbxMissing),
        });
    }
    out
}

/// What the user can do to a row right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Offers {
    pub stop: bool,
    pub delete: bool,
    pub browser: bool,
}

pub(super) fn offers(row: &WorkspaceRow) -> Offers {
    Offers {
        // Only a workspace that is up can be stopped; one already going down needs nothing.
        stop: row.state.can_stop(),
        delete: true,
        browser: row.has_web,
    }
}

/// What the Delete confirmation says before the workspace has been read.
pub(super) const READING_PROMPT: &str = "Checking what would be lost\u{2026}";

/// The checkbox that arms Delete when [`DeletePreview::needs_acknowledgement`].
pub(super) fn acknowledgement_label(row: &WorkspaceRow, preview: &DeletePreview) -> String {
    let noun = row.cloud.noun();
    if !preview.ours {
        format!("I know what is in this {noun}, it is not Monoptah\u{2019}s, and I want it deleted")
    } else {
        format!("I understand what is lost and want this {noun} deleted")
    }
}

/// The paragraphs of the Delete confirmation, from what was read just now.
pub(super) fn delete_prompt(row: &WorkspaceRow, preview: &DeletePreview) -> Vec<String> {
    let noun = row.cloud.noun();
    let mut lines = Vec::new();
    if !preview.ours {
        lines.push(format!(
            "Monoptah did not create \u{201c}{}\u{201d}: another app, another computer or a person made this {noun}, and it may be in use right now.",
            row.display_name
        ));
    }
    lines.push(format!(
        "Delete {noun} {}? This permanently deletes it and everything in it. It cannot be undone.",
        row.display_name
    ));
    if !row.sessions.is_empty() {
        let n = row.sessions.len();
        lines.push(format!(
            "The {n} Monoptah session{} on it will be removed: {}.",
            if n == 1 { "" } else { "s" },
            titles(row)
        ));
    }
    if row.busy() {
        lines.push(
            "A session is working on it right now. Delete is refused until it finishes or is stopped."
                .into(),
        );
    }
    for blocker in &preview.blockers {
        lines.push(format!("Delete is refused: {blocker}"));
    }
    if !preview.lines.is_empty() {
        lines.push("What would be lost:".into());
        lines.extend(preview.lines.iter().map(|line| format!("\u{2022} {line}")));
    }
    lines.extend(preview.notes.iter().cloned());
    if row.cloud.kind == CloudBackendKind::DockerSandbox {
        lines.push(
            "The sandbox is stopped first and deleted once it has stopped; that can take a couple of minutes."
                .into(),
        );
    }
    lines
}

/// What a running action says while it works.
pub(super) fn working_text(cloud: &CloudRef, deleting: bool) -> String {
    match (deleting, cloud.kind) {
        (false, _) => format!("Stopping {}\u{2026}", cloud.noun()),
        (true, CloudBackendKind::DockerSandbox) => {
            "Deleting\u{2026} waiting for the sandbox to stop first".into()
        }
        (true, _) => format!("Deleting {}\u{2026}", cloud.noun()),
    }
}
