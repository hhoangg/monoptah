//! What each connected web tab is looking at, so a push is not sent for the
//! chat the user is reading. Kept in memory only: never saved, never sent to
//! the Worker. A tab reports `{"t":"presence","target":{"kind","id"}|null}`
//! over its Noise session when its page is visible and focused (null when it
//! is not) and repeats it every ~25 s. An entry older than [`STALE_MS`], or
//! one whose session ended, counts for nothing.
use super::session::SessionKey;
use serde_json::Value;
use std::collections::HashMap;

/// A report older than this no longer counts (the web repeats every 25 s).
pub const STALE_MS: u64 = 50_000;

/// What a visible, focused tab shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Session(String),
    Mono(String),
}

impl Target {
    /// The target a web notice is about: a session (including a habit run)
    /// or a Mono's own chat. `None` when it names neither.
    pub fn of_notice(session_id: Option<&str>, mono_id: Option<&str>) -> Option<Target> {
        match (session_id.filter(|id| !id.is_empty()), mono_id) {
            (Some(session), _) => Some(Target::Session(session.to_owned())),
            (None, Some(mono)) => Some(Target::Mono(mono.to_owned())),
            (None, None) => None,
        }
    }
}

/// A presence report decoded from an envelope: `Some(None)` is a cleared
/// state, `None` is not a presence message (or one this build cannot read).
pub fn parse(envelope: &Value) -> Option<Option<Target>> {
    if envelope["t"] != "presence" {
        return None;
    }
    let target = &envelope["target"];
    if target.is_null() {
        return Some(None);
    }
    let id = target["id"]
        .as_str()
        .filter(|id| !id.is_empty())?
        .to_owned();
    match target["kind"].as_str()? {
        "session" => Some(Some(Target::Session(id))),
        "mono" => Some(Some(Target::Mono(id))),
        _ => None,
    }
}

struct Entry {
    /// The relay device id (hyphenated UUID), as `Device::server_device_id`.
    device_id: String,
    target: Target,
    at_ms: u64,
}

/// One entry per session (tab). Clock values are passed in by the caller.
#[derive(Default)]
pub struct PresenceTable {
    entries: HashMap<SessionKey, Entry>,
}

impl PresenceTable {
    /// Records a report from a session; `None` clears it.
    pub fn set(&mut self, key: SessionKey, device_id: &str, target: Option<Target>, now_ms: u64) {
        match target {
            Some(target) => {
                self.entries.insert(
                    key,
                    Entry {
                        device_id: device_id.to_owned(),
                        target,
                        at_ms: now_ms,
                    },
                );
            }
            None => {
                self.entries.remove(&key);
            }
        }
    }

    /// The session ended or reconnected: its report is gone.
    pub fn remove(&mut self, key: &SessionKey) {
        self.entries.remove(key);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Whether any tab of the device (`server_device_id`) reported `target`
    /// within [`STALE_MS`] of `now_ms`.
    pub fn is_viewing(&self, device_id: &str, target: &Target, now_ms: u64) -> bool {
        self.entries.values().any(|entry| {
            entry.device_id == device_id
                && entry.target == *target
                && now_ms.saturating_sub(entry.at_ms) < STALE_MS
        })
    }
}

/// Milliseconds on a process-wide monotonic clock.
pub fn clock_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(device: u8, channel: u16) -> SessionKey {
        SessionKey {
            device_id: [device; 16],
            channel,
        }
    }

    fn session(id: &str) -> Target {
        Target::Session(id.into())
    }

    #[test]
    fn parses_targets_and_the_cleared_state_and_ignores_the_rest() {
        let msg = |target: Value| json!({"t":"presence","version":1,"target":target});
        assert_eq!(
            parse(&msg(json!({"kind":"session","id":"s1"}))),
            Some(Some(session("s1")))
        );
        assert_eq!(
            parse(&msg(json!({"kind":"mono","id":"m1"}))),
            Some(Some(Target::Mono("m1".into())))
        );
        assert_eq!(parse(&msg(Value::Null)), Some(None));
        assert_eq!(parse(&msg(json!({"kind":"other","id":"x"}))), None);
        assert_eq!(parse(&msg(json!({"kind":"mono","id":""}))), None);
        assert_eq!(parse(&json!({"rid":1,"method":"x"})), None);
    }

    #[test]
    fn a_fresh_report_counts_only_for_the_same_device_and_exact_target() {
        let mut table = PresenceTable::default();
        table.set(key(1, 1), "dev-a", Some(session("s1")), 1_000);
        assert!(table.is_viewing("dev-a", &session("s1"), 1_000));
        assert!(!table.is_viewing("dev-b", &session("s1"), 1_000));
        assert!(!table.is_viewing("dev-a", &session("s2"), 1_000));
        assert!(!table.is_viewing("dev-a", &Target::Mono("s1".into()), 1_000));
    }

    #[test]
    fn a_report_goes_stale_and_a_heartbeat_renews_it() {
        let mut table = PresenceTable::default();
        table.set(key(1, 1), "dev-a", Some(session("s1")), 0);
        assert!(table.is_viewing("dev-a", &session("s1"), STALE_MS - 1));
        assert!(!table.is_viewing("dev-a", &session("s1"), STALE_MS));
        table.set(key(1, 1), "dev-a", Some(session("s1")), 40_000);
        assert!(table.is_viewing("dev-a", &session("s1"), 40_000 + STALE_MS - 1));
    }

    #[test]
    fn cleared_removed_and_cleared_all_entries_stop_counting() {
        let mut table = PresenceTable::default();
        table.set(key(1, 1), "dev-a", Some(session("s1")), 0);
        table.set(key(1, 1), "dev-a", None, 1);
        assert!(!table.is_viewing("dev-a", &session("s1"), 2));
        table.set(key(1, 1), "dev-a", Some(session("s1")), 0);
        table.remove(&key(1, 1));
        assert!(!table.is_viewing("dev-a", &session("s1"), 2));
        table.set(key(1, 1), "dev-a", Some(session("s1")), 0);
        table.clear();
        assert!(!table.is_viewing("dev-a", &session("s1"), 2));
    }

    #[test]
    fn any_tab_of_a_device_counts_and_a_hidden_tab_does_not_hide_a_visible_one() {
        let mut table = PresenceTable::default();
        table.set(key(1, 1), "dev-a", Some(session("s1")), 0);
        table.set(key(1, 2), "dev-a", Some(session("s2")), 0);
        table.set(key(1, 2), "dev-a", None, 5);
        assert!(table.is_viewing("dev-a", &session("s1"), 5));
        assert!(!table.is_viewing("dev-a", &session("s2"), 5));
    }

    #[test]
    fn a_new_session_for_the_same_channel_replaces_the_old_report() {
        let mut table = PresenceTable::default();
        table.set(key(1, 1), "dev-a", Some(session("s1")), 0);
        table.set(key(1, 1), "dev-a", Some(session("s2")), 1);
        assert!(!table.is_viewing("dev-a", &session("s1"), 2));
        assert!(table.is_viewing("dev-a", &session("s2"), 2));
    }

    #[test]
    fn a_notice_target_is_the_session_or_else_the_mono() {
        assert_eq!(Target::of_notice(Some("s"), Some("m")), Some(session("s")));
        assert_eq!(
            Target::of_notice(None, Some("m")),
            Some(Target::Mono("m".into()))
        );
        assert_eq!(
            Target::of_notice(Some(""), Some("m")),
            Some(Target::Mono("m".into()))
        );
        assert_eq!(Target::of_notice(None, None), None);
    }

    #[test]
    fn the_link_drops_a_report_when_its_session_ends_or_the_link_is_cleared() {
        let link = super::super::WebLink::default();
        link.presence_set(key(1, 1), "dev-a", Some(session("s1")));
        link.presence_set(key(2, 1), "dev-b", Some(session("s1")));
        assert!(link.presence_is_viewing("dev-a", &session("s1")));
        link.live_remove(&key(1, 1));
        assert!(!link.presence_is_viewing("dev-a", &session("s1")));
        assert!(link.presence_is_viewing("dev-b", &session("s1")));
        link.live_clear();
        assert!(!link.presence_is_viewing("dev-b", &session("s1")));
    }
}
