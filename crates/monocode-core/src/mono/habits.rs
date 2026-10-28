//! The React Mono habit schedule, claiming and run-history policy.
use super::files;
use chrono::{
    DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
pub const MAX: usize = 20;
pub const GRACE_MS: u64 = 2 * 60 * 60 * 1000;
pub const TIMEOUT_MS: u64 = 60 * 60 * 1000;
pub const APPROVAL_WAIT_MS: u64 = 12 * 60 * 60 * 1000;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    #[serde(alias = "kind")]
    pub schedule_kind: String,
    #[serde(default)]
    pub minute: u32,
    #[serde(default = "nine")]
    pub time: String,
    #[serde(default = "monday")]
    pub day_of_week: u32,
    /// Hours between runs of an `interval` schedule (1 to 24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_hours: Option<u8>,
    /// Local "YYYY-MM-DD" the interval slots count from; set by `anchor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_from: Option<String>,
}
const KINDS: [&str; 5] = ["hourly", "daily", "weekdays", "weekly", "interval"];
fn nine() -> String {
    "09:00".into()
}
fn monday() -> u32 {
    1
}
impl Default for Schedule {
    fn default() -> Self {
        Self {
            schedule_kind: "daily".into(),
            minute: 0,
            time: nine(),
            day_of_week: 1,
            interval_hours: None,
            interval_from: None,
        }
    }
}
/// A strict 24-hour "HH:MM".
fn clock(text: &str) -> Option<(u32, u32)> {
    let b = text.as_bytes();
    if b.len() != 5 || b[2] != b':' || ![0, 1, 3, 4].iter().all(|&i| b[i].is_ascii_digit()) {
        return None;
    }
    let (h, m) = (text[..2].parse().ok()?, text[3..].parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}
fn date_shaped(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}
/// A JSON number holding a whole value, like `Number.isInteger`.
fn whole(value: &Value) -> Option<i64> {
    value
        .as_f64()
        .filter(|f| f.fract() == 0.0 && f.abs() < 9e15)
        .map(|f| f as i64)
}
/// A wall-clock time as the machine's local time; a time inside a skipped hour
/// reads with the offset from before the change, so it lands after the gap.
fn local_ms(naive: NaiveDateTime) -> Option<u64> {
    let secs = match Local.from_local_datetime(&naive) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.timestamp(),
        LocalResult::None => {
            let before = Local
                .from_local_datetime(&(naive - Duration::hours(3)))
                .earliest()?;
            naive.and_utc().timestamp() - before.offset().local_minus_utc() as i64
        }
    };
    Some((secs * 1000).max(0) as u64)
}
impl Schedule {
    /// A schedule from the CLI: `{"kind", "time"?, "minute"?, "dayOfWeek"?, "hours"?}`.
    pub fn from_cli(value: &Value) -> Result<Self, String> {
        let input = value
            .as_object()
            .ok_or(r#"schedule must be an object like {"kind":"weekdays","time":"09:00"}"#)?;
        let kind = input
            .get("kind")
            .and_then(Value::as_str)
            .filter(|k| KINDS.contains(k))
            .ok_or("schedule.kind must be hourly, daily, weekdays, weekly or interval")?;
        let interval = kind == "interval";
        let given = |key: &str| input.get(key).filter(|v| !v.is_null());
        let time = match given("time") {
            None => (if interval { "00:00" } else { "09:00" }).to_string(),
            Some(Value::String(t)) if clock(t).is_some() => t.clone(),
            _ => return Err(r#"schedule.time must be 24-hour "HH:MM""#.into()),
        };
        let minute = match given("minute") {
            None => 0,
            Some(v) => whole(v)
                .filter(|m| (0..=59).contains(m))
                .ok_or("schedule.minute must be 0-59")? as u32,
        };
        let day_of_week = match given("dayOfWeek") {
            None => 1,
            Some(v) => whole(v)
                .filter(|d| (0..=6).contains(d))
                .ok_or("schedule.dayOfWeek must be 0 (Sunday) to 6")? as u32,
        };
        let unknown: Vec<&str> = input
            .keys()
            .map(String::as_str)
            .filter(|k| !["kind", "time", "minute", "dayOfWeek", "hours"].contains(k))
            .collect();
        if !unknown.is_empty() {
            return Err(format!("Unknown schedule fields: {}", unknown.join(", ")));
        }
        let hours = input.get("hours");
        if !interval && hours.is_some() {
            return Err(r#"schedule.hours only applies to kind "interval""#.into());
        }
        let interval_hours = if interval {
            let hours = hours.ok_or(
                r#"schedule.hours is required for kind "interval": an integer from 1 to 24"#,
            )?;
            Some(
                whole(hours)
                    .filter(|h| (1..=24).contains(h))
                    .ok_or("schedule.hours must be an integer from 1 to 24")? as u8,
            )
        } else {
            None
        };
        Ok(Self {
            schedule_kind: kind.into(),
            minute,
            time,
            day_of_week,
            interval_hours,
            interval_from: None,
        })
    }
    /// An interval schedule pinned to the local day of `now`, so its first run
    /// falls on the anchor time whatever `interval_hours` is. Others pass through.
    pub fn anchor(mut self, now: u64) -> Self {
        if self.schedule_kind == "interval" {
            if let Some(at) = Local.timestamp_millis_opt(now as i64).single() {
                self.interval_from = Some(at.format("%Y-%m-%d").to_string());
            }
        }
        self
    }
    /// Drops what the kind does not use and an anchor date that is not one.
    fn normalized(mut self) -> Self {
        if self.schedule_kind != "interval" {
            self.interval_hours = None;
            self.interval_from = None;
        } else if self
            .interval_from
            .as_deref()
            .is_some_and(|f| !date_shaped(f))
        {
            self.interval_from = None;
        }
        self
    }
    pub fn validate(&self) -> Result<(), String> {
        if !KINDS.contains(&self.schedule_kind.as_str())
            || clock(&self.time).is_none()
            || self.minute > 59
            || self.day_of_week > 6
            || self.schedule_kind == "interval"
                && !self.interval_hours.is_some_and(|h| (1..=24).contains(&h))
        {
            return Err("Choose hourly, daily, weekdays, weekly or interval, a valid 24-hour time, minute 0-59, weekday 0-6 and 1-24 hours for interval".into());
        }
        Ok(())
    }
    pub fn label(&self) -> String {
        match self.schedule_kind.as_str() {
            "interval" => {
                let hours = self.interval_hours.unwrap_or(24);
                let every = if hours == 1 {
                    "hour".to_string()
                } else {
                    format!("{hours} hours")
                };
                format!("Every {every} from {}", self.time)
            }
            "hourly" => format!("Hourly at :{:02}", self.minute),
            "weekdays" => format!("Weekdays at {}", self.time),
            "weekly" => format!(
                "{} at {}",
                [
                    "Sunday",
                    "Monday",
                    "Tuesday",
                    "Wednesday",
                    "Thursday",
                    "Friday",
                    "Saturday"
                ][self.day_of_week.min(6) as usize],
                self.time
            ),
            _ => format!("Daily at {}", self.time),
        }
    }
    pub fn next(&self, after: u64) -> Result<u64, String> {
        self.validate()?;
        let start = DateTime::from_timestamp_millis(after as i64)
            .ok_or("Invalid schedule date")?
            .with_timezone(&Local);
        if self.schedule_kind == "interval" {
            return Ok(self.next_interval(after));
        }
        let hour = self.time[..2].parse::<u32>().unwrap();
        let minute = self.time[3..].parse::<u32>().unwrap();
        // Iterate real minutes across DST transitions. Wall-clock predicates
        // avoid invalid or duplicated local-time arithmetic.
        let mut at =
            start.with_second(0).unwrap().with_nanosecond(0).unwrap() + Duration::minutes(1);
        for _ in 0..8 * 24 * 60 {
            let day = at.weekday().num_days_from_sunday();
            let matches = if self.schedule_kind == "hourly" {
                at.minute() == self.minute
            } else {
                at.hour() == hour
                    && at.minute() == minute
                    && match self.schedule_kind.as_str() {
                        "weekdays" => (1..=5).contains(&day),
                        "weekly" => day == self.day_of_week,
                        _ => true,
                    }
            };
            if matches {
                return Ok(at.timestamp_millis().max(0) as u64);
            }
            at += Duration::minutes(1);
        }
        Err("No next habit run found".into())
    }
    /// The first slot strictly after `after` of a run every `interval_hours`
    /// from the anchor `time` on the day `interval_from`. Slots are counted on
    /// the local wall clock, so they carry on across midnight and a clock change
    /// does not move them; when the hours do not divide 24 they drift against
    /// the days. An unusable time or date falls back to 00:00 / 1970-01-01.
    fn next_interval(&self, after: u64) -> u64 {
        let hours = self.interval_hours.unwrap_or(24).clamp(1, 24) as i64;
        let (h, m) = clock(&self.time).unwrap_or((0, 0));
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        let from = self
            .interval_from
            .as_deref()
            .filter(|f| date_shaped(f))
            .and_then(|f| NaiveDate::parse_from_str(f, "%Y-%m-%d").ok())
            .unwrap_or(epoch);
        let first = from.signed_duration_since(epoch).num_days() * 1440 + (h * 60 + m) as i64;
        let step = hours * 60;
        let fallback = after + step as u64 * 60_000;
        let Some(wall) = DateTime::from_timestamp_millis(after as i64)
            .map(|t| t.with_timezone(&Local).naive_local())
        else {
            return fallback;
        };
        let now = wall.and_utc().timestamp().div_euclid(60);
        let next_slot = (now - first).div_euclid(step) + 1;
        // A slot in an hour the clock skips lands on the next one, so it can fall
        // at or before `after`; move on rather than go back.
        for slot in next_slot..next_slot + 3 {
            let at = DateTime::from_timestamp((first + slot * step) * 60, 0)
                .and_then(|t| local_ms(t.naive_utc()));
            if let Some(at) = at.filter(|at| *at > after) {
                return at;
            }
        }
        fallback
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Habit {
    pub id: String,
    pub name: String,
    pub instructions: String,
    pub schedule: Schedule,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub next_run_at: u64,
    pub last_run_at: Option<u64>,
    pub last_outcome: Option<String>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub runs: Vec<Run>,
    #[serde(default)]
    pub run_requested: bool,
    #[serde(default)]
    pub running_since: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_card: Option<String>,
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub at: u64,
    #[serde(default)]
    pub duration_ms: u64,
    pub outcome: String,
    pub report: Option<String>,
    pub error: Option<String>,
}
impl Habit {
    pub fn new(
        name: String,
        instructions: String,
        schedule: Schedule,
        now: u64,
    ) -> Result<Self, String> {
        if name.trim().is_empty()
            || name.chars().count() > 120
            || instructions.trim().is_empty()
            || instructions.len() > 32000
        {
            return Err("A habit needs a name and instructions".into());
        }
        let schedule = schedule.anchor(now);
        let next_run_at = schedule.next(now)?;
        Ok(Self {
            id: super::new_id(),
            name: name.trim().into(),
            instructions: instructions.trim().into(),
            schedule,
            enabled: true,
            created_at: now,
            next_run_at,
            last_run_at: None,
            last_outcome: None,
            last_error: None,
            runs: Vec::new(),
            run_requested: false,
            running_since: None,
            from_card: None,
        })
    }
    /// A new schedule counts from `now`; nothing changes when it is unusable.
    pub fn reschedule(&mut self, schedule: Schedule, now: u64) -> Result<(), String> {
        let schedule = schedule.anchor(now);
        let next = schedule.next(now)?;
        self.schedule = schedule;
        self.next_run_at = next;
        Ok(())
    }
    pub fn due(&self, now: u64) -> bool {
        self.running_since.is_none()
            && (self.run_requested
                || self.enabled && self.next_run_at <= now && now - self.next_run_at <= GRACE_MS)
    }
    pub fn finish(&mut self, run: Run) -> Result<(), String> {
        self.last_run_at = Some(run.at);
        self.last_outcome = Some(run.outcome.clone());
        self.last_error = run.error.clone();
        self.running_since = None;
        self.runs.insert(0, run);
        self.runs.truncate(20);
        Ok(())
    }
}
pub fn load(root: &Path, id: &str) -> Result<Vec<Habit>, String> {
    let file = files::read(root, id, None, "habits.json")?;
    parse(file.text.as_deref())
}
/// What habits.json holds: the habits this app can read, and where they sit
/// among entries it cannot (a newer kind, bad hours, anything else), which are
/// kept as they were.
struct Parsed {
    entries: Vec<Entry>,
    habits: Vec<Habit>,
}
enum Entry {
    Habit(String),
    Raw(Value),
}
fn read_habit(raw: &Value) -> Option<Habit> {
    let mut value = raw.clone();
    // The desktop parser accepts each recent run independently. A bad
    // history item must not make the habit (and its controls) disappear.
    if let Some(items) = value.get_mut("runs") {
        let runs: Vec<Run> = items
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|run| serde_json::from_value::<Run>(run.clone()).ok())
            .filter(|run| {
                ["posted", "quiet", "failed", "interrupted"].contains(&run.outcome.as_str())
            })
            .take(20)
            .collect();
        *items = serde_json::to_value(runs).ok()?;
    }
    let mut habit = serde_json::from_value::<Habit>(value).ok()?;
    habit.schedule = habit.schedule.normalized();
    (!habit.id.trim().is_empty()
        && !habit.name.trim().is_empty()
        && !habit.instructions.trim().is_empty()
        && habit.schedule.validate().is_ok())
    .then_some(habit)
}
fn parse_entries(text: Option<&str>) -> Result<Parsed, String> {
    let mut parsed = Parsed {
        entries: Vec::new(),
        habits: Vec::new(),
    };
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return Ok(parsed);
    };
    let values: Vec<Value> =
        serde_json::from_str(text).map_err(|e| format!("Cannot read habits: {e}"))?;
    for raw in values {
        match read_habit(&raw).filter(|_| parsed.habits.len() < MAX) {
            Some(habit) => {
                parsed.entries.push(Entry::Habit(habit.id.clone()));
                parsed.habits.push(habit);
            }
            None => parsed.entries.push(Entry::Raw(raw)),
        }
    }
    Ok(parsed)
}
fn parse(text: Option<&str>) -> Result<Vec<Habit>, String> {
    Ok(parse_entries(text)?.habits)
}
/// Rewrites the file from the edited habits: unread entries stay as they were
/// where they were, habits still present stay in place, new ones go last.
fn merge(entries: Vec<Entry>, habits: &[Habit]) -> Result<Vec<Value>, String> {
    let mut used = vec![false; habits.len()];
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Entry::Raw(raw) => out.push(raw),
            Entry::Habit(id) => {
                if let Some(i) = (0..habits.len()).find(|&i| !used[i] && habits[i].id == id) {
                    used[i] = true;
                    out.push(serde_json::to_value(&habits[i]).map_err(|e| e.to_string())?);
                }
            }
        }
    }
    for (habit, used) in habits.iter().zip(used) {
        if !used {
            out.push(serde_json::to_value(habit).map_err(|e| e.to_string())?);
        }
    }
    Ok(out)
}
pub fn update<T>(
    root: &Path,
    id: &str,
    change: impl Fn(&mut Vec<Habit>) -> Result<T, String>,
) -> Result<T, String> {
    for _ in 0..5 {
        let file = files::read(root, id, None, "habits.json")?;
        let Parsed {
            entries,
            mut habits,
        } = parse_entries(file.text.as_deref())?;
        let result = change(&mut habits)?;
        if habits.len() > MAX {
            return Err("At most 20 habits per Mono".into());
        }
        let text = serde_json::to_string_pretty(&merge(entries, &habits)?)
            .map_err(|e| e.to_string())?
            + "\n";
        match files::save(root, id, None, "habits.json", &text, Some(&file.hash)) {
            Ok(_) => return Ok(result),
            Err(e) if e == files::CONFLICT => continue,
            Err(e) => return Err(e),
        }
    }
    Err("Habits kept changing; try again".into())
}
pub fn report(text: &str) -> Option<String> {
    let t = text.trim();
    let simple = t
        .chars()
        .filter(|c| !c.is_whitespace() && !"`*.".contains(*c))
        .collect::<String>();
    if simple == "NOTHING_TO_REPORT" {
        return None;
    }
    let text = t.trim_end_matches("NOTHING_TO_REPORT").trim();
    (!text.is_empty()).then(|| text.chars().take(4000).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_habit_optional_timestamps_and_partial_history_remain_editable() {
        let text = r#"[{"id":"desktop","name":"Digest","instructions":"Check the news",
            "schedule":{"scheduleKind":"daily"},"fromCard":"suggestion-1",
            "runs":[{"at":100,"outcome":"posted","report":"**News**"},
                    {"at":"bad","outcome":"failed"},
                    {"at":80,"outcome":"quiet"}]}]"#;
        let habits = parse(Some(text)).unwrap();
        assert_eq!(habits.len(), 1);
        let h = &habits[0];
        assert_eq!(h.created_at, 0);
        assert_eq!(h.next_run_at, 0);
        assert_eq!(h.runs.len(), 2);
        assert_eq!(h.runs[0].duration_ms, 0);
        assert_eq!(h.runs[0].report.as_deref(), Some("**News**"));
        assert_eq!(h.from_card.as_deref(), Some("suggestion-1"));
        let serialized = serde_json::to_string(&habits).unwrap();
        assert_eq!(parse(Some(&serialized)).unwrap()[0].from_card, h.from_card);
    }

    #[test]
    fn malformed_history_does_not_hide_an_otherwise_valid_habit() {
        let text = r#"[{"id":"desktop","name":"Digest","instructions":"Check updates",
            "schedule":{"scheduleKind":"hourly","minute":15},"runs":null}]"#;
        let habits = parse(Some(text)).unwrap();
        assert_eq!(habits.len(), 1);
        assert!(habits[0].runs.is_empty());
    }

    use chrono::TimeZone;
    use serde_json::{json, Value};

    const HOUR: u64 = 60 * 60 * 1000;

    fn at(day: u32, hour: u32, minute: u32) -> u64 {
        Local
            .with_ymd_and_hms(2026, 10, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64
    }

    fn interval(hours: u8, time: &str) -> Schedule {
        Schedule {
            schedule_kind: "interval".into(),
            time: time.into(),
            minute: 0,
            day_of_week: 1,
            interval_hours: Some(hours),
            interval_from: None,
        }
    }

    fn habit_with(schedule: Schedule, next_run_at: u64) -> Habit {
        let mut h = Habit::new("n".into(), "i".into(), schedule, at(5, 8, 0)).unwrap();
        h.next_run_at = next_run_at;
        h
    }

    #[test]
    fn validates_interval_schedules_from_the_cli() {
        assert_eq!(
            Schedule::from_cli(&json!({"kind":"interval","hours":4,"time":"09:00"})).unwrap(),
            interval(4, "09:00")
        );
        // The anchor defaults to midnight.
        for hours in [1u8, 6, 24] {
            assert_eq!(
                Schedule::from_cli(&json!({"kind":"interval","hours":hours})).unwrap(),
                interval(hours, "00:00")
            );
        }
        for hours in [
            json!(0),
            json!(25),
            json!(-3),
            json!(1.5),
            json!("4"),
            Value::Null,
        ] {
            assert_eq!(
                Schedule::from_cli(&json!({"kind":"interval","hours":hours})).unwrap_err(),
                "schedule.hours must be an integer from 1 to 24"
            );
        }
        assert!(Schedule::from_cli(&json!({"kind":"interval"}))
            .unwrap_err()
            .contains("schedule.hours is required"));
        for time in ["9am", "24:00"] {
            assert!(
                Schedule::from_cli(&json!({"kind":"interval","hours":4,"time":time}))
                    .unwrap_err()
                    .contains("HH:MM")
            );
        }
        assert_eq!(
            Schedule::from_cli(&json!({"kind":"interval","hours":4,"cron":"* *"})).unwrap_err(),
            "Unknown schedule fields: cron"
        );
        assert_eq!(
            Schedule::from_cli(&json!({"kind":"daily","hours":4})).unwrap_err(),
            r#"schedule.hours only applies to kind "interval""#
        );
        assert_eq!(
            Schedule::from_cli(&json!({"kind":"monthly"})).unwrap_err(),
            "schedule.kind must be hourly, daily, weekdays, weekly or interval"
        );
        assert!(Schedule::from_cli(&json!("daily"))
            .unwrap_err()
            .starts_with("schedule must be an object"));
        assert_eq!(
            Schedule::from_cli(&json!({"kind":"hourly","minute":60})).unwrap_err(),
            "schedule.minute must be 0-59"
        );
        assert_eq!(
            Schedule::from_cli(&json!({"kind":"weekly","dayOfWeek":7})).unwrap_err(),
            "schedule.dayOfWeek must be 0 (Sunday) to 6"
        );
        // The other kinds are unchanged and carry no interval.
        let daily = Schedule::from_cli(&json!({"kind":"daily","time":"08:00"})).unwrap();
        assert_eq!(daily.interval_hours, None);
        assert_eq!(daily.time, "08:00");
        assert!(serde_json::to_value(&daily)
            .unwrap()
            .get("intervalHours")
            .is_none());
        assert_eq!(
            Schedule::from_cli(&json!({"kind":"weekdays"}))
                .unwrap()
                .time,
            "09:00"
        );
    }

    #[test]
    fn runs_an_interval_habit_at_the_anchor_and_every_n_hours_after_it() {
        let s = interval(4, "09:00");
        assert_eq!(s.next(at(5, 8, 0)).unwrap(), at(5, 9, 0));
        // Strictly after: a run that just fired moves on to the next slot.
        assert_eq!(s.next(at(5, 9, 0)).unwrap(), at(5, 13, 0));
        assert_eq!(s.next(at(5, 9, 0) + 30_000).unwrap(), at(5, 13, 0));
        assert_eq!(s.next(at(5, 17, 1)).unwrap(), at(5, 21, 0));
        // It continues across midnight rather than restarting at the anchor.
        assert_eq!(s.next(at(5, 21, 0)).unwrap(), at(6, 1, 0));
        assert_eq!(s.next(at(6, 1, 0)).unwrap(), at(6, 5, 0));
        assert_eq!(s.next(at(6, 5, 0)).unwrap(), at(6, 9, 0));
        // Slots before the anchor on its own day count too.
        assert_eq!(s.next(at(5, 0, 0)).unwrap(), at(5, 1, 0));
        assert_eq!(
            interval(1, "00:30").next(at(5, 9, 31)).unwrap(),
            at(5, 10, 30)
        );
        assert_eq!(
            interval(6, "09:15").next(at(5, 9, 15)).unwrap(),
            at(5, 15, 15)
        );
        // Every 24 hours is a daily habit.
        assert_eq!(
            interval(24, "07:00").next(at(5, 7, 0)).unwrap(),
            at(6, 7, 0)
        );
    }

    #[test]
    fn keeps_the_interval_steady_when_n_does_not_divide_24() {
        let s = interval(5, "09:00");
        let mut runs = Vec::new();
        let mut t = at(5, 0, 0);
        while runs.len() < 25 {
            t = s.next(t).unwrap();
            runs.push(t);
        }
        // Always N hours apart, across midnights (no DST change in these days).
        for pair in runs.windows(2) {
            assert_eq!(pair[1] - pair[0], 5 * HOUR);
        }
        // The same slot comes back every 5 days (120 hours), and so does the anchor.
        assert!((5..=9).any(|d| runs.contains(&at(d, 9, 0))));
        assert_eq!(runs[24] - runs[0], 120 * HOUR);
        // Where it lands does not depend on when it is asked.
        assert_eq!(s.next(runs[3] - 1).unwrap(), runs[3]);
        assert_eq!(s.next(runs[3] - 4 * HOUR).unwrap(), runs[3]);
        let seven = interval(7, "22:00");
        let first = seven.next(at(5, 12, 0)).unwrap();
        assert_eq!(seven.next(first).unwrap(), first + 7 * HOUR);
    }

    #[test]
    fn starts_a_new_interval_habit_on_its_anchor_whatever_n_is() {
        let make = |hours: u8, now: u64| {
            Habit::new("n".into(), "i".into(), interval(hours, "09:00"), now).unwrap()
        };
        // Made before the anchor on any day, the first run is the anchor itself.
        for hours in [1u8, 4, 5, 7, 9, 11, 24] {
            for day in 5..=14 {
                assert_eq!(make(hours, at(day, 8, 0)).next_run_at, at(day, 9, 0));
            }
        }
        // Made after the anchor, it runs on from the anchor's own sequence.
        let made = make(5, at(5, 10, 0));
        assert_eq!(made.schedule.interval_from.as_deref(), Some("2026-10-05"));
        assert_eq!(made.next_run_at, at(5, 14, 0));
        let second = made.schedule.next(made.next_run_at).unwrap();
        assert_eq!(second, at(5, 19, 0));
        assert_eq!(made.schedule.next(second).unwrap(), at(6, 0, 0));
        // Other kinds are not pinned to a day.
        let daily = Habit::new("n".into(), "i".into(), Schedule::default(), at(5, 8, 0)).unwrap();
        assert_eq!(daily.schedule.interval_from, None);
        // The pin survives habits.json, and a bad one is ignored, not fatal.
        let text = serde_json::to_string(&[&made]).unwrap();
        let back = parse(Some(&text)).unwrap();
        assert_eq!(back[0].schedule, made.schedule);
        let garbled = text.replace("2026-10-05", "yesterday");
        let back = parse(Some(&garbled)).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].schedule.interval_from, None);
        assert_eq!(back[0].schedule.interval_hours, Some(5));
    }

    #[test]
    fn an_unusable_interval_anchor_is_kept_unread_and_never_hangs() {
        let saved = habit_with(interval(4, "09:00"), 0);
        for time in ["1e20:00", "25:99", "9:5", "", "noon"] {
            let mut broken = serde_json::to_value(&saved).unwrap();
            broken["schedule"]["time"] = json!(time);
            let text = serde_json::to_string(&[&broken]).unwrap();
            assert!(parse(Some(&text)).unwrap().is_empty());
            // Built in code rather than read from disk, it still gives a sane time.
            let mut s = saved.schedule.clone();
            s.time = time.into();
            assert!(s.next(at(5, 8, 0)).is_err());
            let next = s.next_interval(at(5, 8, 0));
            assert!(next > at(5, 8, 0));
            assert!(next <= at(5, 8, 0) + 4 * HOUR);
        }
        // Unusable hours are not guessed either.
        for hours in [json!(0), json!(25), json!(1.5), json!("4"), json!(null)] {
            let mut broken = serde_json::to_value(&saved).unwrap();
            broken["schedule"]["intervalHours"] = hours;
            let text = serde_json::to_string(&[&broken]).unwrap();
            assert!(parse(Some(&text)).unwrap().is_empty());
        }
    }

    #[test]
    fn catches_an_interval_habit_up_once_and_never_queues_a_backlog() {
        let s = interval(4, "09:00");
        let due = habit_with(s.clone(), at(5, 13, 0));
        // 10 minutes late: runs once, then waits for the next slot.
        let now = at(5, 13, 10);
        assert!(due.due(now));
        assert_eq!(s.next(now).unwrap(), at(5, 17, 0));
        // Asleep through 17:00 and 21:00 and woken at 21:30: one run, not three.
        let late = habit_with(s.clone(), at(5, 17, 0));
        let woke = at(5, 21, 30);
        assert!(!late.due(woke));
        assert_eq!(s.next(woke).unwrap(), at(6, 1, 0));
        // Within the grace period the missed slot still runs, once.
        let recent = habit_with(s.clone(), at(5, 17, 0));
        assert!(recent.due(at(5, 18, 30)));
        assert_eq!(s.next(at(5, 18, 30)).unwrap(), at(5, 21, 0));
        // A new habit is first due at its next slot.
        assert_eq!(
            Habit::new("n".into(), "i".into(), s, at(5, 10, 0))
                .unwrap()
                .next_run_at,
            at(5, 13, 0)
        );
    }

    #[test]
    fn labels_interval_schedules_readably() {
        assert_eq!(interval(4, "09:00").label(), "Every 4 hours from 09:00");
        assert_eq!(interval(1, "00:00").label(), "Every hour from 00:00");
        assert_eq!(interval(24, "07:30").label(), "Every 24 hours from 07:30");
    }

    #[test]
    fn interval_habits_round_trip_the_desktop_file_shape() {
        let text = r#"[{"id":"i1","name":"CI","instructions":"Watch CI",
            "schedule":{"scheduleKind":"interval","minute":0,"time":"09:00","dayOfWeek":1,
                        "intervalHours":4,"intervalFrom":"2026-10-05"},
            "enabled":true,"createdAt":1,"nextRunAt":2}]"#;
        let habits = parse(Some(text)).unwrap();
        assert_eq!(habits.len(), 1);
        let s = &habits[0].schedule;
        assert_eq!(
            (s.interval_hours, s.interval_from.as_deref()),
            (Some(4), Some("2026-10-05"))
        );
        let out = serde_json::to_value(&habits[0]).unwrap();
        assert_eq!(out["schedule"]["intervalHours"], 4);
        assert_eq!(out["schedule"]["intervalFrom"], "2026-10-05");
        assert_eq!(out["schedule"]["scheduleKind"], "interval");
    }

    #[test]
    fn habits_add_and_update_round_trip_through_the_cli_shape() {
        let temp = tempfile::tempdir().unwrap();
        let (root, id) = (temp.path(), "interval-cli");
        // The clock is 08:00, so the first slot is the 09:00 anchor.
        let now = at(5, 8, 0);
        let schedule = Schedule::from_cli(&json!({"kind":"interval","hours":4,"time":"09:00"}));
        let added = Habit::new(
            "CI watch".into(),
            "Tell me if CI is red.".into(),
            schedule.unwrap(),
            now,
        )
        .unwrap();
        assert_eq!(added.schedule.label(), "Every 4 hours from 09:00");
        assert_eq!(added.next_run_at, at(5, 9, 0));
        let hid = added.id.clone();
        update(root, id, |all| {
            all.push(added.clone());
            Ok(())
        })
        .unwrap();
        // A new schedule is anchored on the day it is set and next is recomputed.
        let five =
            Schedule::from_cli(&json!({"kind":"interval","hours":5,"time":"09:00"})).unwrap();
        update(root, id, |all| all[0].reschedule(five.clone(), now)).unwrap();
        let h = &load(root, id).unwrap()[0];
        assert_eq!(h.id, hid);
        assert_eq!(h.schedule.interval_hours, Some(5));
        assert_eq!(h.schedule.interval_from.as_deref(), Some("2026-10-05"));
        assert_eq!(h.next_run_at, at(5, 9, 0));
        // Too often is refused before anything is written.
        let err = Schedule::from_cli(&json!({"kind":"interval","hours":25})).unwrap_err();
        assert!(err.contains("schedule.hours"));
        // Back to a plain kind drops the interval.
        let daily = Schedule::from_cli(&json!({"kind":"daily","time":"08:30"})).unwrap();
        update(root, id, |all| all[0].reschedule(daily.clone(), now)).unwrap();
        let file = files::read(root, id, None, "habits.json")
            .unwrap()
            .text
            .unwrap();
        let value: Value = serde_json::from_str(&file).unwrap();
        assert!(value[0]["schedule"].get("intervalHours").is_none());
        assert!(value[0]["schedule"].get("intervalFrom").is_none());
        assert_eq!(value[0]["schedule"]["scheduleKind"], "daily");
    }

    #[test]
    fn rewriting_habits_keeps_entries_this_app_cannot_read_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let (root, id) = (temp.path(), "keep-unread");
        let readable = json!({"id":"a","name":"A","instructions":"x",
            "schedule":{"scheduleKind":"interval","minute":0,"time":"09:00","dayOfWeek":1,
                        "intervalHours":4,"intervalFrom":"2026-10-05"},
            "enabled":true,"createdAt":1,"nextRunAt":2,"lastRunAt":null,"lastOutcome":null,
            "lastError":null,"runs":[],"runRequested":false,"runningSince":null});
        // A kind from a newer app, an unusable interval, a non-object, and a habit
        // missing required fields: none of them can be read here.
        let future = json!({"id":"f","name":"F","instructions":"y","extra":{"keep":[1,2.5,"z"]},
            "schedule":{"scheduleKind":"monthly","time":"09:00","dayOfMonth":3}});
        let zero = json!({"id":"z","name":"Z","instructions":"y",
            "schedule":{"scheduleKind":"interval","time":"09:00","intervalHours":0}});
        let junk = json!(42);
        let nameless = json!({"id":"n","instructions":"y","schedule":{"scheduleKind":"daily"}});
        let original = json!([future, readable, zero, junk, nameless]);
        files::save(root, id, None, "habits.json", &original.to_string(), None).unwrap();
        assert_eq!(load(root, id).unwrap().len(), 1);
        let b = Habit::new("B".into(), "z".into(), Schedule::default(), at(5, 8, 0)).unwrap();
        update(root, id, |all| {
            all[0].enabled = false;
            all.push(b.clone());
            Ok(())
        })
        .unwrap();
        let text = files::read(root, id, None, "habits.json")
            .unwrap()
            .text
            .unwrap();
        let saved: Vec<Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(saved.len(), 6);
        // Unread entries are written back as they were, in their original places.
        assert_eq!(saved[0], future);
        assert_eq!(saved[2], zero);
        assert_eq!(saved[3], junk);
        assert_eq!(saved[4], nameless);
        // The readable interval habit survives the edit, and the new one goes last.
        assert_eq!(saved[1]["enabled"], false);
        assert_eq!(saved[1]["schedule"]["intervalHours"], 4);
        assert_eq!(saved[1]["schedule"]["intervalFrom"], "2026-10-05");
        assert_eq!(saved[5]["id"], json!(b.id));
        // Removing the readable one leaves the unread ones alone.
        update(root, id, |all| {
            all.retain(|h| h.id != "a");
            Ok(())
        })
        .unwrap();
        let text = files::read(root, id, None, "habits.json")
            .unwrap()
            .text
            .unwrap();
        let saved: Vec<Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(saved.len(), 5);
        assert_eq!(saved[0], future);
        assert_eq!(saved[1], zero);
    }

    #[test]
    fn habits_past_the_limit_are_not_dropped_by_a_rewrite() {
        let temp = tempfile::tempdir().unwrap();
        let (root, id) = (temp.path(), "over-limit");
        let many: Vec<Value> = (0..MAX + 3)
            .map(|n| {
                json!({"id":format!("h{n}"),"name":"N","instructions":"x",
                "schedule":{"scheduleKind":"daily","time":"09:00"}})
            })
            .collect();
        files::save(
            root,
            id,
            None,
            "habits.json",
            &json!(many).to_string(),
            None,
        )
        .unwrap();
        assert_eq!(load(root, id).unwrap().len(), MAX);
        update(root, id, |all| {
            all[0].enabled = false;
            Ok(())
        })
        .unwrap();
        let text = files::read(root, id, None, "habits.json")
            .unwrap()
            .text
            .unwrap();
        let saved: Vec<Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(saved.len(), MAX + 3);
        assert_eq!(saved[MAX + 2], many[MAX + 2]);
    }

    #[test]
    fn interval_slots_stay_ordered_across_clock_changes_in_any_timezone() {
        // Spring forward and fall back in the US, plus the EU's fall back: the
        // result depends on the machine's zone, but never goes back or stalls.
        for (month, day) in [(3, 6), (10, 31), (3, 27), (10, 24)] {
            for hours in [1u8, 2, 5, 24] {
                let mut s = interval(hours, "02:30");
                let start = Local.with_ymd_and_hms(2026, month, day, 0, 0, 0).unwrap();
                s.interval_from = Some(start.format("%Y-%m-%d").to_string());
                let mut t = start.timestamp_millis() as u64;
                for _ in 0..40 {
                    let next = s.next(t).unwrap();
                    assert!(next > t);
                    assert!(next - t <= hours as u64 * HOUR + 2 * HOUR);
                    t = next;
                }
            }
        }
    }
}
