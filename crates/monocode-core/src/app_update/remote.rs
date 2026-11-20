//! Remote update: the rules shared by the `app.update` Mono action and the
//! `desktop.update` relay method. Pure data and decisions, no I/O besides the
//! settings row that remembers a request across the restart.
//!
//! Security: the caller names nothing but a mode and a confirmation. The
//! version and the package always come from the signed feed, and an update
//! is only ever offered when it is newer than the running version.
use crate::store::Store;
pub use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The settings row that holds a [`Pending`] request while the app restarts.
pub const PENDING_KEY: &str = "monocode.remoteUpdate";
/// Longest failure reason written into a chat or a notification.
const REASON_CHARS: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Check,
    Install,
}

/// What a remote caller asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub mode: Mode,
    /// Install although sessions are running (the restart interrupts them).
    pub force: bool,
}

/// Reads the input of the `app.update` action. `mode` defaults to `check`;
/// `force` and `confirm` both mean the same thing.
pub fn parse_request(input: &Value) -> Result<Request, String> {
    let mode = match input.get("mode").and_then(Value::as_str) {
        None | Some("check") => Mode::Check,
        Some("install") => Mode::Install,
        Some(_) => return Err("mode must be \"check\" or \"install\"".into()),
    };
    let flag = |key: &str| input.get(key).and_then(Value::as_bool).unwrap_or(false);
    Ok(Request {
        mode,
        force: flag("force") || flag("confirm"),
    })
}

/// A session that a restart would interrupt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningSession {
    pub id: String,
    pub title: String,
}

/// The newest package of the feed that is newer than the running app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub version: Version,
    pub notes: String,
}

/// The answer to `check`, also the status the web settings screen shows.
/// `latest` equals `current` when nothing newer exists.
pub fn status(
    current: &Version,
    offer: Option<&Offer>,
    running: &[RunningSession],
    error: Option<&str>,
) -> Value {
    let newer = offer.filter(|offer| offer.version > *current);
    let mut value = json!({
        "current": current.to_string(),
        "latest": newer.map_or_else(|| current.to_string(), |o| o.version.to_string()),
        "updateAvailable": newer.is_some(),
        "notes": newer.map(|o| o.notes.as_str()).unwrap_or_default(),
        "runningSessions": running,
    });
    if let Some(error) = error {
        value["error"] = json!(capped(error));
    }
    value
}

/// What an install request does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing newer exists. Never a downgrade.
    UpToDate { current: String },
    /// Sessions are running and the caller did not confirm.
    Confirm { sessions: Vec<RunningSession> },
    /// Download, install and restart.
    Proceed { from: String, to: String },
}

pub fn decide(
    current: &Version,
    offer: Option<&Offer>,
    running: &[RunningSession],
    force: bool,
) -> Decision {
    let Some(offer) = offer.filter(|offer| offer.version > *current) else {
        return Decision::UpToDate {
            current: current.to_string(),
        };
    };
    if !running.is_empty() && !force {
        return Decision::Confirm {
            sessions: running.to_vec(),
        };
    }
    Decision::Proceed {
        from: current.to_string(),
        to: offer.version.to_string(),
    }
}

impl Decision {
    /// The reply to the caller. It is sent before anything is installed.
    pub fn reply(&self) -> Value {
        match self {
            Decision::UpToDate { current } => json!({
                "status": "up_to_date",
                "current": current,
                "message": format!("Monoptah is up to date ({current})."),
            }),
            Decision::Confirm { sessions } => json!({
                "status": "confirmation_required",
                "runningSessions": sessions,
                "message": "Sessions are running and a restart interrupts them. Ask the user, then repeat with force true (confirm true for a device).",
            }),
            Decision::Proceed { from, to } => json!({
                "status": "updating",
                "from": from,
                "to": to,
                "message": format!("Updating to {to}: the app downloads it, installs it and restarts. The result is posted to this chat after the restart."),
            }),
        }
    }
}

/// Who asked, kept across the restart so the result can be reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub from: String,
    pub to: String,
    /// `mono` or `device`.
    pub kind: String,
    /// The Mono's or the device's name, for the desktop notice.
    pub requester: String,
    /// The Mono whose chat gets the result, when a Mono asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    pub requested_at: u64,
}

impl Pending {
    pub fn save(&self, store: &Store) -> Result<(), String> {
        store.save_setting(
            PENDING_KEY,
            &serde_json::to_string(self).map_err(|e| e.to_string())?,
        )
    }
    pub fn load(store: &Store) -> Result<Option<Self>, String> {
        let Some(raw) = store.setting(PENDING_KEY)? else {
            return Ok(None);
        };
        // An unreadable row is dropped rather than reported again at every launch.
        Ok(serde_json::from_str(&raw).ok())
    }
    pub fn clear(store: &Store) -> Result<(), String> {
        store.delete_setting(PENDING_KEY)
    }
}

/// What became of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Updated,
    Failed(String),
}

impl Outcome {
    /// The message for the requesting chat and the notifications.
    pub fn message(&self, pending: &Pending) -> String {
        match self {
            Outcome::Updated => format!("Updated to {}", pending.to),
            Outcome::Failed(reason) => {
                format!("Update to {} failed: {}", pending.to, capped(reason))
            }
        }
    }
}

/// Judges a request left by the previous run by the version running now.
pub fn resolve(pending: &Pending, running: &str) -> Outcome {
    if running == pending.to {
        Outcome::Updated
    } else {
        Outcome::Failed(format!(
            "the app restarted on version {running} instead of {}",
            pending.to
        ))
    }
}

fn capped(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(REASON_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }
    fn offer(text: &str) -> Offer {
        Offer {
            version: v(text),
            notes: "Fixes".into(),
        }
    }
    fn session() -> RunningSession {
        RunningSession {
            id: "7".into(),
            title: "Fix tests".into(),
        }
    }
    fn pending() -> Pending {
        Pending {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
            kind: "mono".into(),
            requester: "Ada".into(),
            chat: Some("mono-1".into()),
            requested_at: 5,
        }
    }

    #[test]
    fn request_defaults_to_check_and_reads_force_or_confirm() {
        let parse = |value: Value| parse_request(&value);
        assert_eq!(
            parse(json!({})).unwrap(),
            Request {
                mode: Mode::Check,
                force: false
            }
        );
        assert_eq!(
            parse(json!({"mode": "install", "force": true})).unwrap(),
            Request {
                mode: Mode::Install,
                force: true
            }
        );
        assert!(
            parse(json!({"mode": "install", "confirm": true}))
                .unwrap()
                .force
        );
        assert!(parse(json!({"mode": "downgrade"})).is_err());
    }

    #[test]
    fn status_reports_current_latest_and_notes() {
        let value = status(&v("0.1.0"), Some(&offer("0.2.0")), &[session()], None);
        assert_eq!(value["current"], "0.1.0");
        assert_eq!(value["latest"], "0.2.0");
        assert_eq!(value["updateAvailable"], true);
        assert_eq!(value["notes"], "Fixes");
        assert_eq!(value["runningSessions"][0]["title"], "Fix tests");
        let none = status(&v("0.1.0"), None, &[], Some("feed offline"));
        assert_eq!(none["latest"], "0.1.0");
        assert_eq!(none["updateAvailable"], false);
        assert_eq!(none["error"], "feed offline");
    }

    #[test]
    fn an_older_or_equal_feed_never_installs() {
        for feed in ["0.1.0", "0.0.9"] {
            let offer = offer(feed);
            assert_eq!(
                status(&v("0.1.0"), Some(&offer), &[], None)["updateAvailable"],
                false
            );
            assert_eq!(
                decide(&v("0.1.0"), Some(&offer), &[], true),
                Decision::UpToDate {
                    current: "0.1.0".into()
                }
            );
        }
        assert_eq!(
            decide(&v("0.1.0"), None, &[], true).reply()["status"],
            "up_to_date"
        );
    }

    #[test]
    fn running_sessions_need_a_confirmation() {
        let running = [session()];
        let held = decide(&v("0.1.0"), Some(&offer("0.2.0")), &running, false);
        assert_eq!(
            held,
            Decision::Confirm {
                sessions: running.to_vec()
            }
        );
        assert_eq!(held.reply()["status"], "confirmation_required");
        assert_eq!(held.reply()["runningSessions"][0]["id"], "7");
        let forced = decide(&v("0.1.0"), Some(&offer("0.2.0")), &running, true);
        assert_eq!(
            forced,
            Decision::Proceed {
                from: "0.1.0".into(),
                to: "0.2.0".into()
            }
        );
        assert_eq!(forced.reply()["status"], "updating");
        assert_eq!(
            decide(&v("0.1.0"), Some(&offer("0.2.0")), &[], false).reply()["to"],
            "0.2.0"
        );
    }

    #[test]
    fn prerelease_versions_order_by_semver() {
        let current = v("0.16.0-preview.1");
        assert!(matches!(
            decide(&current, Some(&offer("0.16.0-preview.2")), &[], false),
            Decision::Proceed { .. }
        ));
    }

    #[test]
    fn the_next_launch_judges_by_the_running_version() {
        let pending = pending();
        assert_eq!(resolve(&pending, "0.2.0"), Outcome::Updated);
        assert_eq!(Outcome::Updated.message(&pending), "Updated to 0.2.0");
        let failed = resolve(&pending, "0.1.0");
        assert_eq!(
            failed.message(&pending),
            "Update to 0.2.0 failed: the app restarted on version 0.1.0 instead of 0.2.0"
        );
    }

    #[test]
    fn a_failure_reason_is_one_short_clean_line() {
        let reason = format!("bad\nnews {}", "x".repeat(900));
        let text = Outcome::Failed(reason).message(&pending());
        assert!(!text.contains('\n'));
        assert!(text.chars().count() < 340);
    }

    #[test]
    fn the_pending_request_survives_in_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        assert_eq!(Pending::load(&store).unwrap(), None);
        pending().save(&store).unwrap();
        assert_eq!(Pending::load(&store).unwrap(), Some(pending()));
        Pending::clear(&store).unwrap();
        assert_eq!(Pending::load(&store).unwrap(), None);
        store.save_setting(PENDING_KEY, "not json").unwrap();
        assert_eq!(Pending::load(&store).unwrap(), None);
    }
}
