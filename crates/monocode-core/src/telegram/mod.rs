//! Telegram question bridge: forwards unanswered agent questions to a Telegram group and
//! routes the answers back. `poller.rs` holds the runtime, this file the app-facing surface.

pub mod api;
pub mod config;
pub mod forward;
pub mod forwarder;
pub mod pending;
pub mod poller;
pub mod queue;
pub mod render;

pub use poller::{Bridge, ConnectLink, EventSink, LinkCode, TelegramStatus, ANSWER_EVENT};
pub use render::{ForwardPayload, OptionIn, QuestionIn};

use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Loads `<data_dir>/telegram-config.json` without starting any thread.
pub fn open(base: &str, data_dir: &Path, sink: Arc<dyn EventSink>) -> Arc<Bridge> {
    Bridge::new(base, config::config_path(data_dir), sink)
}

/// The process-wide bridge. One bridge owns one sender thread, one poller per bot token and
/// the config file: a second one would double-poll a bot (Telegram answers 409) and the two
/// would overwrite each other's `telegram-config.json`.
static BRIDGE: OnceLock<Arc<Bridge>> = OnceLock::new();

fn init_once(
    cell: &OnceLock<Arc<Bridge>>,
    base: &str,
    data_dir: &Path,
    sink: Arc<dyn EventSink>,
) -> Arc<Bridge> {
    Arc::clone(cell.get_or_init(|| {
        let bridge = open(base, data_dir, sink);
        bridge.start();
        bridge
    }))
}

/// Loads the saved config, starts the sender thread and the pollers, once per process.
/// Later calls return the same bridge and ignore their arguments: a different `data_dir` or
/// `sink` does not replace the first ones.
///
/// Blocking: these calls must run on a background executor, never on the UI thread.
/// - `set_token` and `remove_webhook` talk to Telegram.
/// - `set_token`, `set_enabled`, `set_delay` and `disconnect` persist the config (with an
///   fsync) while holding the state mutex. Background threads persist under the same lock,
///   so even `status` can wait on an fsync.
pub fn init(data_dir: &Path, sink: Arc<dyn EventSink>) -> Arc<Bridge> {
    init_once(&BRIDGE, poller::API_BASE, data_dir, sink)
}

pub fn status(bridge: &Bridge, project: &str) -> TelegramStatus {
    bridge.status(project)
}

/// Talks to Telegram: call it off the UI thread.
pub fn set_token(
    bridge: &Arc<Bridge>,
    project: &str,
    token: &str,
) -> Result<TelegramStatus, String> {
    bridge.set_token(project, token)
}

pub fn connect_link(bridge: &Arc<Bridge>, project: &str) -> Result<ConnectLink, String> {
    bridge.connect_link(project)
}

pub fn set_enabled(
    bridge: &Arc<Bridge>,
    project: &str,
    enabled: bool,
) -> Result<TelegramStatus, String> {
    bridge.set_enabled(project, enabled)
}

pub fn set_delay(
    bridge: &Bridge,
    project: &str,
    delay_secs: u64,
) -> Result<TelegramStatus, String> {
    bridge.set_delay(project, delay_secs)
}

pub fn disconnect(bridge: &Arc<Bridge>, project: &str) {
    bridge.disconnect(project);
}

/// Talks to Telegram: call it off the UI thread.
pub fn remove_webhook(bridge: &Arc<Bridge>, project: &str) -> Result<(), String> {
    bridge.remove_webhook(project)
}

pub fn link_code(bridge: &Bridge, project: &str, session_id: &str) -> Result<LinkCode, String> {
    bridge.link_code(project, session_id)
}

pub fn session_topic(bridge: &Bridge, project: &str, session_id: &str) -> Option<i64> {
    bridge.session_topic(project, session_id)
}

/// `window` is the label the answer event is emitted to through the `EventSink`. The label is
/// not validated here: the caller must pass one its sink recognises, because when `emit_to`
/// returns false the answer counts as failed.
pub fn forward_question(
    bridge: &Bridge,
    window: &str,
    payload: ForwardPayload,
) -> Result<(), String> {
    bridge.forward(window, payload)
}

/// `status` is the line shown on the Telegram message once the desktop settled the question.
pub fn question_closed(
    bridge: &Bridge,
    session_id: &str,
    request_id: u64,
    status: &str,
) -> Result<(), String> {
    if !matches!(status, "Answered on desktop" | "Skipped" | "Cancelled") {
        return Err("Unknown question status".to_string());
    }
    bridge.question_closed(session_id, request_id, status);
    Ok(())
}

/// The caller's verdict on an answer event it received through the `EventSink`.
pub fn answer_result(bridge: &Bridge, key: &str, ok: bool) {
    bridge.answer_result(key, ok);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::Arc;

    struct NoSink;

    impl EventSink for NoSink {
        fn emit_to(&self, _window: &str, _event: &str, _payload: Value) -> bool {
            false
        }
    }

    fn bridge(dir: &std::path::Path) -> Arc<Bridge> {
        open("http://127.0.0.1:1", dir, Arc::new(NoSink))
    }

    #[test]
    fn init_once_returns_the_same_bridge_and_keeps_the_first_sink() {
        let (first_dir, second_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let cell = OnceLock::new();
        let (first_sink, second_sink) = (Arc::new(NoSink), Arc::new(NoSink));
        let first = init_once(
            &cell,
            "http://127.0.0.1:1",
            first_dir.path(),
            first_sink.clone(),
        );
        let second = init_once(
            &cell,
            "http://127.0.0.1:1",
            second_dir.path(),
            second_sink.clone(),
        );
        assert!(Arc::ptr_eq(&first, &second));
        // The first sink is held by the bridge; the second was dropped, so no second bridge
        // (and no second sender thread) was built.
        assert_eq!(Arc::strong_count(&first_sink), 2);
        assert_eq!(Arc::strong_count(&second_sink), 1);
    }

    #[test]
    fn open_reads_the_shared_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config::TelegramConfig::default();
        cfg.projects.insert(
            "/p".into(),
            config::ProjectConfig {
                token: "1:abc".into(),
                chat_id: Some(-5),
                ..Default::default()
            },
        );
        config::save(&config::config_path(dir.path()), &cfg).unwrap();
        assert!(dir.path().join("telegram-config.json").exists());
        let bridge = bridge(dir.path());
        assert!(status(&bridge, "/p").configured);
        assert!(!status(&bridge, "/other").configured);
    }

    #[test]
    fn delay_errors_keep_the_tauri_copy() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = bridge(dir.path());
        assert_eq!(
            set_delay(&bridge, "/p", 3601).unwrap_err(),
            "The delay can be at most 3600 seconds"
        );
        assert_eq!(
            set_delay(&bridge, "/p", 60).unwrap_err(),
            "Save a bot token first"
        );
    }

    #[test]
    fn question_closed_rejects_unknown_status() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = bridge(dir.path());
        assert_eq!(
            question_closed(&bridge, "s", 1, "Done").unwrap_err(),
            "Unknown question status"
        );
        for status in ["Answered on desktop", "Skipped", "Cancelled"] {
            assert!(question_closed(&bridge, "s", 1, status).is_ok());
        }
    }

    #[test]
    fn disconnect_and_answer_result_are_safe_without_state() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = bridge(dir.path());
        disconnect(&bridge, "/p");
        answer_result(&bridge, "missing", true);
        assert_eq!(session_topic(&bridge, "/p", "s"), None);
    }
}
