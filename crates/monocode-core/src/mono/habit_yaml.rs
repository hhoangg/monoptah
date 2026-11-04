//! Sharing a habit as a file: the YAML export and the import a user reviews before the habit can
//! run. The file reuses the workflow file machinery (YAML writer and reader, schedule shape,
//! size limit, absolute path rule) and holds only what a habit is: name, schedule and
//! instructions. Nothing machine specific is written (no id, runs, timestamps or enabled flag).
use super::habits::{Habit, Schedule};
use super::workflows::ordered::Json;
use super::workflows::template::js_trim;
use super::workflows::validate::find_absolute_path;
use super::workflows::yaml::{describe_version, export_schedule, parse_file_as};
use super::workflows::yaml_emit::{stringify, Doc};

/// The value of the top-level `kind` key.
pub const HABIT_KIND: &str = "habit";
/// The value of the top-level `version` key.
pub const HABIT_FILE_VERSION: u32 = 1;
const NAME_MAX_CHARS: usize = 120;
const INSTRUCTIONS_MAX_BYTES: usize = 32000;
/// Distinct paths and URLs listed in the warnings before the rest is summarised.
const LISTED_MAX: usize = 5;
const KEYS: [&str; 5] = ["kind", "version", "name", "schedule", "instructions"];

/// What the user is shown before a habit file becomes a habit.
#[derive(Clone, Debug, PartialEq)]
pub struct HabitPreview {
    pub name: String,
    pub instructions: String,
    pub schedule: Schedule,
    /// Things worth a second look, such as a path on someone's machine or a URL.
    pub warnings: Vec<String>,
}

/// The habit as a shareable file. The interval anchor is left out: it is set on import.
pub fn export_habit(habit: &Habit) -> String {
    let schedule = Schedule {
        interval_from: None,
        ..habit.schedule.clone()
    };
    stringify(&Doc::Map(vec![
        ("kind".into(), Doc::Str(HABIT_KIND.into())),
        ("version".into(), Doc::Num(HABIT_FILE_VERSION.into())),
        ("name".into(), Doc::Str(habit.name.clone())),
        ("schedule".into(), export_schedule(&schedule)),
        ("instructions".into(), Doc::Str(habit.instructions.clone())),
    ]))
}

fn describe(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        other => other.stringify(false),
    }
}

const WORKFLOW_FILE: &str = "This is a workflow file; import it in Workflows";

/// Reads YAML or JSON. Refuses what cannot be a habit; warns about what a reader should check.
pub fn preview_habit_import(text: &str) -> Result<HabitPreview, String> {
    let Json::Object(file) = parse_file_as(text, "habit")? else {
        return Err("Not a Monoptah habit file".into());
    };
    match file.get("kind") {
        Some(Json::String(kind)) if kind == HABIT_KIND => {}
        Some(Json::String(kind)) if kind == "workflow" => return Err(WORKFLOW_FILE.into()),
        Some(other) => {
            return Err(format!(
                "Unsupported kind: {} (expected {HABIT_KIND})",
                describe(other)
            ))
        }
        None if file.contains_key("monoptahWorkflow") => return Err(WORKFLOW_FILE.into()),
        None => return Err("Not a Monoptah habit file".into()),
    }
    if file.get("version") != Some(&Json::Number(HABIT_FILE_VERSION.into())) {
        return Err(format!(
            "Unsupported habit file version: {} (this app reads version {HABIT_FILE_VERSION})",
            describe_version(file.get("version"))
        ));
    }
    let unknown: Vec<&str> = file
        .keys()
        .map(String::as_str)
        .filter(|k| !KEYS.contains(k))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "Unknown fields in the habit file: {}",
            unknown.join(", ")
        ));
    }
    let name = match file.get("name") {
        Some(Json::String(name)) if !js_trim(name).is_empty() => js_trim(name).to_string(),
        _ => return Err("The habit file needs a name".into()),
    };
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(format!(
            "The habit name is too long (over {NAME_MAX_CHARS} characters)"
        ));
    }
    let instructions = match file.get("instructions") {
        Some(Json::String(text)) if !js_trim(text).is_empty() => js_trim(text).to_string(),
        _ => return Err("The habit file needs instructions".into()),
    };
    if instructions.len() > INSTRUCTIONS_MAX_BYTES {
        return Err(format!(
            "The habit instructions are too long (over {INSTRUCTIONS_MAX_BYTES} bytes)"
        ));
    }
    let schedule = file
        .get("schedule")
        .ok_or("The habit file needs a schedule")?;
    let schedule = Schedule::from_cli(&schedule.to_value())?;
    schedule.validate()?;
    let warnings = warnings_for(&instructions);
    Ok(HabitPreview {
        name,
        instructions,
        schedule,
        warnings,
    })
}

/// The habit a preview becomes: saved off and waiting for the user's review.
pub fn import_habit(preview: &HabitPreview, now: u64) -> Result<Habit, String> {
    let mut habit = Habit::new(
        preview.name.clone(),
        preview.instructions.clone(),
        preview.schedule.clone(),
        now,
    )?;
    habit.enabled = false;
    habit.needs_review = true;
    Ok(habit)
}

fn is_url(word: &str) -> bool {
    word.split_once("://").is_some_and(|(scheme, rest)| {
        !rest.is_empty()
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    })
}

fn listed(label: &str, found: Vec<String>) -> Option<String> {
    if found.is_empty() {
        return None;
    }
    let more = found.len().saturating_sub(LISTED_MAX);
    let mut shown = found[..found.len() - more].join(", ");
    if more > 0 {
        shown.push_str(&format!(" and {more} more"));
    }
    Some(format!("The instructions mention {label}: {shown}"))
}

/// Things in the instructions worth a second look: absolute paths and URLs.
pub fn warnings_for(instructions: &str) -> Vec<String> {
    let (mut paths, mut urls) = (Vec::<String>::new(), Vec::<String>::new());
    for word in instructions.split_whitespace() {
        let word = word.trim_matches(|c: char| "()[]<>\"'`,;.:".contains(c));
        let bucket = if is_url(word) {
            &mut urls
        } else if find_absolute_path(word).is_some() {
            &mut paths
        } else {
            continue;
        };
        if !bucket.iter().any(|seen| seen == word) {
            bucket.push(word.to_string());
        }
    }
    [
        listed("an absolute path on someone's machine", paths),
        listed("a URL", urls),
    ]
    .into_iter()
    .flatten()
    .collect()
}
