//! Busy/idle signal for terminal sessions, fed by Claude Code hooks.
//!
//! A terminal session runs the real CLI in a PTY, so no adapter reports its
//! turns. The launch registers hooks whose command is this executable:
//! `<exe> tui-hook --spool <dir>`. Each hook invocation drops one small JSON
//! file into the spool directory, and a background thread in the running app
//! turns those files into `tui-hook` events for every window.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

/// Event name every window listens on.
const EVENT: &str = "tui-hook";
const POLL: Duration = Duration::from_millis(300);
/// Age limit for a `busy: true` event only. A turn's start is drained within
/// one poll of being written, so an older one is a leftover from a process that
/// died: replaying it would mark a restored session busy with no CLI left to
/// ever send `Stop`. Dropping it is safe, the worst case is a missed spinner.
///
/// `drain` is direction-aware: it never age-gates a `busy: false` event. The
/// session is already busy in the UI and nothing else clears it (no watchdog,
/// no timeout), so dropping a late `Stop` would leave it busy until another
/// full turn, the pane closing or an app restart. A late one is real: the
/// machine can suspend between the hook writing and the next poll, and the
/// wall clock keeps running (or steps forward) across the sleep. Applying a
/// stale idle is harmless, since the session at worst is already idle.
///
/// The startup `prune` in `init` is not direction-aware: it deletes any record
/// older than this by mtime, idle ones included. That is safe because every
/// session is idle at boot, so a dropped `busy: false` loses nothing.
const MAX_EVENT_AGE: Duration = Duration::from_secs(5);
/// Hook payloads are small; refuse to buffer anything absurd.
const MAX_STDIN: u64 = 1024 * 1024;

/// One state change for the conversation with this provider session id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TuiHookEvent {
    pub provider_session_id: String,
    pub busy: bool,
}

/// Maps a hook payload to a state change, or `None` when it carries none.
/// `Notification` only ends a turn when it is the idle prompt: a permission
/// prompt or a request for input means the turn is still waiting on the user.
pub fn classify(payload: &Value) -> Option<TuiHookEvent> {
    let session_id = payload.get("session_id")?.as_str()?.trim();
    if session_id.is_empty() {
        return None;
    }
    let busy = match payload.get("hook_event_name")?.as_str()? {
        "UserPromptSubmit" => true,
        "Stop" | "StopFailure" | "SessionEnd" => false,
        "Notification" => {
            if payload.get("notification_type")?.as_str()? != "idle_prompt" {
                return None;
            }
            false
        }
        _ => return None,
    };
    Some(TuiHookEvent {
        provider_session_id: session_id.to_string(),
        busy,
    })
}

/// What a spool file holds: the event plus when the hook wrote it. A file
/// without `atMs` (an older build wrote it) cannot be proven fresh. Under the
/// direction rule in `MAX_EVENT_AGE` that means an untimestamped `busy: true`
/// is dropped and an untimestamped `busy: false` is still applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpoolRecord {
    #[serde(flatten)]
    event: TuiHookEvent,
    #[serde(default)]
    at_ms: Option<u64>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Names sort by wall-clock nanoseconds, so files drain in write order. Every
/// hook is its own process, so the pid is only there to keep two hooks that
/// land on the same nanosecond from overwriting each other; it does not order
/// them, and such a tie is broken arbitrarily. That is a real but tiny risk:
/// hooks for one session fire seconds apart.
fn file_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:020}-{:010}.json", nanos, std::process::id())
}

/// Writes the event to a temp name, then renames it, so a reader never sees a
/// partial file. The temp name does not end in `.json`.
pub fn write_event(spool: &Path, event: &TuiHookEvent) -> std::io::Result<()> {
    write_record(spool, &file_name(), event, now_ms())
}

fn write_record(spool: &Path, name: &str, event: &TuiHookEvent, at_ms: u64) -> std::io::Result<()> {
    fs::create_dir_all(spool)?;
    let record = SpoolRecord {
        event: event.clone(),
        at_ms: Some(at_ms),
    };
    let tmp = spool.join(format!(".{name}.tmp"));
    fs::write(&tmp, serde_json::to_vec(&record)?)?;
    fs::rename(&tmp, spool.join(name)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn spool_arg(args: &[String]) -> Option<PathBuf> {
    let at = args.iter().position(|arg| arg == "--spool")?;
    args.get(at + 1)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The `tui-hook` subcommand. Always returns 0 and prints nothing: stdout from
/// some hook events is fed back into the model's context, and a hook must never
/// get in the way of the CLI.
pub fn run(args: Vec<String>) -> i32 {
    let Some(spool) = spool_arg(&args) else {
        return 0;
    };
    let mut input = String::new();
    if std::io::stdin()
        .take(MAX_STDIN)
        .read_to_string(&mut input)
        .is_err()
    {
        return 0;
    }
    if let Some(event) = serde_json::from_str::<Value>(&input)
        .ok()
        .as_ref()
        .and_then(classify)
    {
        let _ = write_event(&spool, &event);
    }
    0
}

/// Where the app reads hook files; also the path the frontend hands to the CLI.
pub struct TuiHookSpool(PathBuf);

#[tauri::command]
pub fn tui_hook_spool_dir(spool: tauri::State<'_, TuiHookSpool>) -> Result<String, String> {
    fs::create_dir_all(&spool.0).map_err(|error| error.to_string())?;
    Ok(spool.0.to_string_lossy().into_owned())
}

fn prune(spool: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(spool) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Reads and removes every complete event file, oldest first. A `busy: true`
/// older than `MAX_EVENT_AGE` at `now_ms`, or without a timestamp, is removed
/// without being returned. A `busy: false` is returned whatever its age. See
/// `MAX_EVENT_AGE` for why the directions differ.
fn drain(spool: &Path, now_ms: u64) -> Vec<TuiHookEvent> {
    let Ok(entries) = fs::read_dir(spool) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    let mut events = Vec::new();
    for path in files {
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(record) = serde_json::from_slice::<SpoolRecord>(&bytes) {
                let fresh = record.at_ms.is_some_and(|at| {
                    now_ms.saturating_sub(at) <= MAX_EVENT_AGE.as_millis() as u64
                });
                if !record.event.busy || fresh {
                    events.push(record.event);
                }
            }
        }
        let _ = fs::remove_file(&path);
    }
    events
}

pub fn init(app: &AppHandle) -> Result<(), String> {
    let spool = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("tui-hooks");
    fs::create_dir_all(&spool).map_err(|error| error.to_string())?;
    // Before the first drain, so a leftover never reaches the poll thread. Not
    // direction-aware: an old `busy: false` is deleted too, which is safe
    // because every session is idle at boot.
    prune(&spool, MAX_EVENT_AGE);
    app.manage(TuiHookSpool(spool.clone()));
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(POLL);
        for event in drain(&spool, now_ms()) {
            // Every window: the hook knows only a provider session id, so each
            // window decides whether it owns that session.
            let _ = app.emit(EVENT, event);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(payload: Value) -> Option<(String, bool)> {
        classify(&payload).map(|e| (e.provider_session_id, e.busy))
    }

    #[test]
    fn prompt_submit_is_busy() {
        let got = event(json!({"session_id": "s1", "hook_event_name": "UserPromptSubmit"}));
        assert_eq!(got, Some(("s1".into(), true)));
    }

    #[test]
    fn stop_events_are_idle() {
        for name in ["Stop", "StopFailure", "SessionEnd"] {
            let got = event(json!({"session_id": "s1", "hook_event_name": name}));
            assert_eq!(got, Some(("s1".into(), false)), "{name}");
        }
    }

    #[test]
    fn only_the_idle_notification_ends_a_turn() {
        let idle = json!({"session_id": "s1", "hook_event_name": "Notification", "notification_type": "idle_prompt"});
        assert_eq!(event(idle), Some(("s1".into(), false)));
        for kind in ["permission_prompt", "agent_needs_input", "auth_success"] {
            let other = json!({"session_id": "s1", "hook_event_name": "Notification", "notification_type": kind});
            assert_eq!(event(other), None, "{kind}");
        }
        let untyped = json!({"session_id": "s1", "hook_event_name": "Notification"});
        assert_eq!(event(untyped), None);
    }

    #[test]
    fn unrelated_or_malformed_payloads_are_ignored() {
        assert_eq!(
            event(json!({"session_id": "s1", "hook_event_name": "PreToolUse"})),
            None
        );
        assert_eq!(event(json!({"hook_event_name": "Stop"})), None);
        assert_eq!(
            event(json!({"session_id": "", "hook_event_name": "Stop"})),
            None
        );
        assert_eq!(event(json!({"session_id": "s1"})), None);
        assert_eq!(event(json!([1, 2])), None);
    }

    fn ev(busy: bool) -> TuiHookEvent {
        TuiHookEvent {
            provider_session_id: "s1".into(),
            busy,
        }
    }

    fn temp_spool(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tui-hook-{tag}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn spool_round_trip_is_ordered_and_consumed() {
        let dir = temp_spool("rt");
        // Explicit names: ordering comes from the name, which production
        // derives from the clock, so the test pins it instead of racing it.
        write_record(
            &dir,
            "00000000000000000001-0000000001.json",
            &ev(true),
            1000,
        )
        .unwrap();
        write_record(
            &dir,
            "00000000000000000002-0000000001.json",
            &ev(false),
            1000,
        )
        .unwrap();
        // A half-written temp file must be invisible to the reader.
        fs::write(dir.join(".partial.tmp"), b"{").unwrap();
        assert_eq!(drain(&dir, 1000), vec![ev(true), ev(false)]);
        assert!(drain(&dir, 1000).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_event_is_drained_while_fresh() {
        let dir = temp_spool("fresh");
        write_event(&dir, &ev(true)).unwrap();
        assert_eq!(drain(&dir, now_ms()), vec![ev(true)]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_busy_true_is_dropped_and_removed() {
        let dir = temp_spool("stale-busy");
        let age = MAX_EVENT_AGE.as_millis() as u64;
        write_record(&dir, "1-1.json", &ev(true), 10_000).unwrap();
        assert!(drain(&dir, 10_000 + age + 1).is_empty());
        assert!(!dir.join("1-1.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_busy_false_is_still_emitted_and_removed() {
        let dir = temp_spool("stale-idle");
        write_record(&dir, "1-1.json", &ev(false), 10_000).unwrap();
        // Minutes later, as after a laptop sleep or a clock step.
        assert_eq!(drain(&dir, 10_000 + 600_000), vec![ev(false)]);
        assert!(!dir.join("1-1.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_busy_true_is_emitted_at_the_age_limit() {
        let dir = temp_spool("edge");
        let age = MAX_EVENT_AGE.as_millis() as u64;
        write_record(&dir, "1-1.json", &ev(true), 10_000).unwrap();
        assert_eq!(drain(&dir, 10_000 + age), vec![ev(true)]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_without_a_timestamp_follows_the_direction_rule() {
        let dir = temp_spool("legacy");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("1-1.json"),
            br#"{"providerSessionId":"s1","busy":true}"#,
        )
        .unwrap();
        fs::write(
            dir.join("2-1.json"),
            br#"{"providerSessionId":"s1","busy":false}"#,
        )
        .unwrap();
        fs::write(dir.join("3-1.json"), b"not json").unwrap();
        assert_eq!(drain(&dir, now_ms()), vec![ev(false)]);
        assert!(!dir.join("1-1.json").exists());
        assert!(!dir.join("2-1.json").exists());
        assert!(!dir.join("3-1.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_removes_only_old_files() {
        let dir = std::env::temp_dir().join(format!("tui-hook-prune-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.json"), b"{}").unwrap();
        prune(&dir, Duration::from_secs(3600));
        assert!(dir.join("a.json").exists());
        std::thread::sleep(Duration::from_millis(20));
        prune(&dir, Duration::from_millis(1));
        assert!(!dir.join("a.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spool_arg_needs_a_value() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            spool_arg(&args(&["--spool", "/a b"])),
            Some(PathBuf::from("/a b"))
        );
        assert_eq!(spool_arg(&args(&["--spool"])), None);
        assert_eq!(spool_arg(&args(&[])), None);
    }
}
