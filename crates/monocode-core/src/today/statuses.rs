//! Workflow statuses: the user's named, colored columns, each in one of three
//! stages. One ordered set for the whole app, stored as a setting. Reconcile and
//! every lifecycle rule read only the stage, never the name.
use super::{CheckReason, Status, Task};
use serde::{Deserialize, Serialize};

/// Where a status sits in a task's life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    NotStarted,
    Started,
    Finished,
}
impl Stage {
    pub const ALL: [Stage; 3] = [Stage::NotStarted, Stage::Started, Stage::Finished];
}

pub const TODO_ID: &str = "todo";
pub const IN_PROGRESS_ID: &str = "in_progress";
pub const DONE_ID: &str = "done";
/// Longest status name, in characters.
pub const MAX_NAME: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStatus {
    pub id: String,
    pub name: String,
    pub stage: Stage,
    /// `#rrggbb`, lowercase.
    pub color: String,
    /// One of the three defaults: can be renamed and recolored, never deleted
    /// and never moved to another stage.
    #[serde(default)]
    pub builtin: bool,
}
impl WorkflowStatus {
    /// The color as `0xRRGGBB`; a malformed one reads as the stage's first preset.
    pub fn rgb(&self) -> u32 {
        parse_color(&self.color).unwrap_or(presets(self.stage)[0])
    }
}

/// What a status edit may change. Stage and order change through a move.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusPatch {
    pub name: Option<String>,
    /// `#rrggbb`.
    pub color: Option<String>,
}

/// The id of the built-in status of `stage`.
pub fn builtin_id(stage: Stage) -> &'static str {
    match stage {
        Stage::NotStarted => TODO_ID,
        Stage::Started => IN_PROGRESS_ID,
        Stage::Finished => DONE_ID,
    }
}

/// The three preset swatches the status dialog offers for `stage`.
pub fn presets(stage: Stage) -> [u32; 3] {
    match stage {
        Stage::NotStarted => [0x8b8b8b, 0xe5a23c, 0xf472b6],
        Stage::Started => [0x4da3f5, 0xa78bfa, 0x22d3ee],
        Stage::Finished => [0x10b981, 0x84cc16, 0x64748b],
    }
}

/// To do, In progress, Done.
pub fn default_statuses() -> Vec<WorkflowStatus> {
    Stage::ALL
        .iter()
        .map(|&stage| WorkflowStatus {
            id: builtin_id(stage).into(),
            name: match stage {
                Stage::NotStarted => "To do",
                Stage::Started => "In progress",
                Stage::Finished => "Done",
            }
            .into(),
            stage,
            color: format_color(presets(stage)[0]),
            builtin: true,
        })
        .collect()
}

/// `#rrggbb` (any case) to `0xRRGGBB`.
pub fn parse_color(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// `0xRRGGBB` to `#rrggbb`.
pub fn format_color(rgb: u32) -> String {
    format!("#{:06x}", rgb & 0xff_ffff)
}

/// Repairs a stored list: drops entries with an empty or repeated id, puts back
/// any missing built-in (with its default name and color), forces built-ins to
/// their own stage, and orders by stage (Not started, Started, Finished)
/// keeping the relative order inside a stage. Never fails.
pub fn normalize(list: Vec<WorkflowStatus>) -> Vec<WorkflowStatus> {
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<WorkflowStatus> = Vec::new();
    for mut status in list {
        if status.id.is_empty() || !seen.insert(status.id.clone()) {
            continue;
        }
        if let Some(stage) = Stage::ALL
            .iter()
            .copied()
            .find(|&stage| builtin_id(stage) == status.id)
        {
            status.stage = stage;
            status.builtin = true;
        } else {
            status.builtin = false;
        }
        if status.name.trim().is_empty() {
            status.name = default_name(status.stage).into();
        }
        if parse_color(&status.color).is_none() {
            status.color = format_color(presets(status.stage)[0]);
        } else {
            status.color = status.color.to_ascii_lowercase();
        }
        kept.push(status);
    }
    let restored: Vec<WorkflowStatus> = default_statuses()
        .into_iter()
        .filter(|default| !seen.contains(&default.id))
        .collect();
    // Stable by stage; a built-in put back leads its stage.
    let mut out = Vec::with_capacity(kept.len() + restored.len());
    for stage in Stage::ALL {
        out.extend(restored.iter().filter(|s| s.stage == stage).cloned());
        out.extend(kept.iter().filter(|s| s.stage == stage).cloned());
    }
    out
}

fn default_name(stage: Stage) -> &'static str {
    match stage {
        Stage::NotStarted => "To do",
        Stage::Started => "In progress",
        Stage::Finished => "Done",
    }
}

const NOT_STARTED_WORDS: &[&str] = &[
    "not started",
    "to do",
    "todo",
    "open",
    "backlog",
    "new",
    "ready",
    "pending",
    "reopened",
];
const STARTED_WORDS: &[&str] = &[
    "in progress",
    "in review",
    "review",
    "doing",
    "started",
    "active",
    "wip",
    "blocked",
    "draft",
];
const FINISHED_WORDS: &[&str] = &[
    "closed",
    "merged",
    "done",
    "complete",
    "completed",
    "resolved",
    "finished",
    "shipped",
    "cancelled",
    "canceled",
];

/// The stage a source's own status text means, or `None` when it is unknown.
/// Open, to do, backlog, new and the like are Not started; in progress, in review,
/// doing and the like are Started; closed, merged, done, resolved and the like are
/// Finished. Case-insensitive; "not started" is Not started.
pub fn stage_of_source_status(text: &str) -> Option<Stage> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        return None;
    }
    let tables: [(&[&str], Stage); 3] = [
        (NOT_STARTED_WORDS, Stage::NotStarted),
        (STARTED_WORDS, Stage::Started),
        (FINISHED_WORDS, Stage::Finished),
    ];
    // The longest phrase wins ("not started" beats "started"); a negated match
    // ("not done") means nothing.
    let mut best: Option<(usize, Stage)> = None;
    for (table, stage) in tables {
        for phrase in table {
            let parts: Vec<&str> = phrase.split(' ').collect();
            if parts.len() > words.len() {
                continue;
            }
            for start in 0..=words.len() - parts.len() {
                if words[start..start + parts.len()]
                    .iter()
                    .map(String::as_str)
                    .ne(parts.iter().copied())
                {
                    continue;
                }
                let negated = start > 0 && matches!(words[start - 1].as_str(), "not" | "non");
                if negated && *phrase != "not started" {
                    continue;
                }
                if best.is_none_or(|(len, _)| parts.len() > len) {
                    best = Some((parts.len(), stage));
                }
            }
        }
    }
    best.map(|(_, stage)| stage)
}

/// The status a task shows, from `statuses` (normalized). A task whose
/// `status_id` is empty (saved before statuses existed) or names no status any
/// more is mapped from its lifecycle: `Done`, or a `NeedsCheck` raised by
/// [`CheckReason::DoneButSourceOpen`], is the built-in Done; anything else is the
/// built-in of the stage its `source_status` maps to (Started only; else To do).
pub fn resolve<'a>(task: &Task, statuses: &'a [WorkflowStatus]) -> &'a WorkflowStatus {
    if let Some(found) = statuses.iter().find(|s| s.id == task.status_id) {
        return found;
    }
    let finished = task.status == Status::Done
        || (task.status == Status::NeedsCheck
            && task.check_reason == Some(CheckReason::DoneButSourceOpen));
    let stage = if finished {
        Stage::Finished
    } else if stage_of_source_status(&task.source_status) == Some(Stage::Started) {
        Stage::Started
    } else {
        Stage::NotStarted
    };
    builtin(statuses, stage)
}

/// The built-in status of `stage` in `statuses`, or the default one when the list
/// lacks it (it never does after [`normalize`]).
pub fn builtin(statuses: &[WorkflowStatus], stage: Stage) -> &WorkflowStatus {
    let id = builtin_id(stage);
    statuses.iter().find(|s| s.id == id).unwrap_or_else(|| {
        static FALLBACK: std::sync::OnceLock<Vec<WorkflowStatus>> = std::sync::OnceLock::new();
        let defaults = FALLBACK.get_or_init(default_statuses);
        defaults.iter().find(|s| s.id == id).expect("default")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(id: &str, name: &str, stage: Stage) -> WorkflowStatus {
        WorkflowStatus {
            id: id.into(),
            name: name.into(),
            stage,
            color: format_color(presets(stage)[1]),
            builtin: false,
        }
    }

    fn ids(list: &[WorkflowStatus]) -> Vec<&str> {
        list.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn normalize_of_nothing_is_the_defaults() {
        assert_eq!(normalize(Vec::new()), default_statuses());
    }

    #[test]
    fn normalize_puts_back_missing_builtins_and_orders_by_stage() {
        let list = vec![
            custom("c-done", "Shipped", Stage::Finished),
            custom("c-start", "Review", Stage::Started),
            custom("c-todo", "Backlog", Stage::NotStarted),
        ];
        let got = normalize(list);
        assert_eq!(
            ids(&got),
            ["todo", "c-todo", "in_progress", "c-start", "done", "c-done"]
        );
    }

    #[test]
    fn normalize_keeps_order_inside_a_stage_and_drops_bad_ids() {
        let list = vec![
            custom("b", "B", Stage::Started),
            custom("", "Nameless id", Stage::Started),
            custom("a", "A", Stage::Started),
            custom("b", "Again", Stage::Started),
        ];
        let got = normalize(list);
        let started: Vec<_> = got
            .iter()
            .filter(|s| s.stage == Stage::Started)
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(started, ["in_progress", "b", "a"]);
    }

    #[test]
    fn normalize_forces_builtins_to_their_stage() {
        let mut moved = default_statuses();
        moved[0].stage = Stage::Finished;
        moved[0].builtin = false;
        let got = normalize(moved);
        let todo = got.iter().find(|s| s.id == "todo").unwrap();
        assert_eq!(todo.stage, Stage::NotStarted);
        assert!(todo.builtin);
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn normalize_keeps_a_renamed_builtin() {
        let mut list = default_statuses();
        list[1].name = "Doing".into();
        list[1].color = "#112233".into();
        let got = normalize(list);
        let status = got.iter().find(|s| s.id == "in_progress").unwrap();
        assert_eq!(
            (status.name.as_str(), status.color.as_str()),
            ("Doing", "#112233")
        );
    }

    #[test]
    fn source_status_maps_by_stage() {
        for text in [
            "open",
            "To Do",
            "to do",
            "TODO",
            "Backlog",
            "new",
            "Ready",
            "pending",
            "Not Started",
            "reopened",
        ] {
            assert_eq!(
                stage_of_source_status(text),
                Some(Stage::NotStarted),
                "{text}"
            );
        }
        for text in [
            "In Progress",
            "in review",
            "Review",
            "doing",
            "started",
            "Active",
            "WIP",
            "blocked",
            "draft",
            "  in   progress ",
        ] {
            assert_eq!(stage_of_source_status(text), Some(Stage::Started), "{text}");
        }
        for text in [
            "closed",
            "Merged",
            "DONE",
            "complete",
            "Completed",
            "resolved",
            "finished",
            "shipped",
            "cancelled",
            "Canceled",
        ] {
            assert_eq!(
                stage_of_source_status(text),
                Some(Stage::Finished),
                "{text}"
            );
        }
    }

    #[test]
    fn source_status_unknown_text_is_none() {
        for text in ["", "   ", "Wontfix", "QA sign off", "Unknown"] {
            assert_eq!(stage_of_source_status(text), None, "{text}");
        }
    }

    #[test]
    fn source_status_matches_words_inside_longer_text() {
        assert_eq!(
            stage_of_source_status("Code Review - waiting"),
            Some(Stage::Started)
        );
        assert_eq!(
            stage_of_source_status("Not started yet"),
            Some(Stage::NotStarted)
        );
        assert_eq!(stage_of_source_status("PR merged"), Some(Stage::Finished));
        // A word inside another word is not a match.
        assert_eq!(stage_of_source_status("Opening"), None);
        assert_eq!(stage_of_source_status("Not done"), None);
    }

    fn task(status: Status) -> Task {
        Task {
            status,
            ..Task::default()
        }
    }

    #[test]
    fn resolve_uses_the_status_id_when_it_exists() {
        let mut list = default_statuses();
        list.insert(1, custom("c1", "Next", Stage::NotStarted));
        let list = normalize(list);
        let t = Task {
            status_id: "c1".into(),
            ..task(Status::Open)
        };
        assert_eq!(resolve(&t, &list).name, "Next");
    }

    #[test]
    fn resolve_maps_a_task_without_a_status_id_from_its_lifecycle() {
        let list = default_statuses();
        assert_eq!(resolve(&task(Status::Open), &list).id, "todo");
        assert_eq!(resolve(&task(Status::Done), &list).id, "done");
        let check = Task {
            check_reason: Some(CheckReason::DoneButSourceOpen),
            ..task(Status::NeedsCheck)
        };
        assert_eq!(resolve(&check, &list).id, "done");
        let missing = Task {
            check_reason: Some(CheckReason::Missing),
            ..task(Status::NeedsCheck)
        };
        assert_eq!(resolve(&missing, &list).id, "todo");
        for status in [Status::Snoozed, Status::Dismissed] {
            assert_eq!(resolve(&task(status), &list).id, "todo");
        }
    }

    #[test]
    fn resolve_maps_a_started_source_status_to_in_progress() {
        let t = Task {
            source_status: "In Review".into(),
            ..task(Status::Open)
        };
        assert_eq!(resolve(&t, &default_statuses()).id, "in_progress");
        let finished_text = Task {
            source_status: "Done".into(),
            ..task(Status::Open)
        };
        assert_eq!(resolve(&finished_text, &default_statuses()).id, "todo");
    }

    #[test]
    fn resolve_falls_back_when_the_status_is_gone() {
        let t = Task {
            status_id: "deleted".into(),
            ..task(Status::Done)
        };
        assert_eq!(resolve(&t, &default_statuses()).id, "done");
    }

    #[test]
    fn color_round_trips() {
        assert_eq!(parse_color("#10B981"), Some(0x10b981));
        assert_eq!(format_color(0x10b981), "#10b981");
        assert_eq!(parse_color("10b981"), None);
        assert_eq!(parse_color("#10b98"), None);
        assert_eq!(parse_color("#10b98g"), None);
    }
}
