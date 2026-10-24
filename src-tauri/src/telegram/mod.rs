//! Telegram question bridge: forwards unanswered agent questions to a Telegram group and
//! routes the answers back. `poller.rs` holds the runtime, this file the Tauri surface.

pub mod api;
pub mod config;
pub mod pending;
pub mod poller;
pub mod queue;
pub mod render;

use poller::{Bridge, ConnectLink, EventSink, LinkCode, TelegramStatus};
use render::ForwardPayload;
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

/// Emits answer events to a specific window, and reports when that window is gone.
struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn emit_to(&self, window: &str, event: &str, payload: Value) -> bool {
        self.0.get_webview_window(window).is_some()
            && self.0.emit_to(window, event, payload).is_ok()
    }
}

/// Loads the saved config, starts the sender thread and the pollers.
pub fn init(app: &AppHandle) {
    let dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(_) => {
            eprintln!("monoptah: telegram is off because the app data directory is unknown");
            return;
        }
    };
    let bridge = Bridge::new(
        poller::API_BASE,
        config::config_path(&dir),
        Arc::new(TauriSink(app.clone())),
    );
    app.manage(Arc::clone(&bridge));
    bridge.start();
}

type Shared<'a> = State<'a, Arc<Bridge>>;

/// Runs a blocking bridge call (it talks to Telegram) off the async runtime.
async fn blocking<T: Send + 'static>(
    bridge: &Shared<'_>,
    work: impl FnOnce(Arc<Bridge>) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let bridge = Arc::clone(bridge.inner());
    tauri::async_runtime::spawn_blocking(move || work(bridge))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn telegram_status(bridge: Shared<'_>, project: String) -> TelegramStatus {
    bridge.status(&project)
}

#[tauri::command]
pub async fn telegram_set_token(
    bridge: Shared<'_>,
    project: String,
    token: String,
) -> Result<TelegramStatus, String> {
    blocking(&bridge, move |bridge| bridge.set_token(&project, &token)).await
}

#[tauri::command]
pub fn telegram_connect_link(bridge: Shared<'_>, project: String) -> Result<ConnectLink, String> {
    bridge.connect_link(&project)
}

#[tauri::command]
pub fn telegram_set_enabled(
    bridge: Shared<'_>,
    project: String,
    enabled: bool,
) -> Result<TelegramStatus, String> {
    bridge.set_enabled(&project, enabled)
}

#[tauri::command]
pub fn telegram_set_delay(
    bridge: Shared<'_>,
    project: String,
    delay_secs: u64,
) -> Result<TelegramStatus, String> {
    bridge.set_delay(&project, delay_secs)
}

#[tauri::command]
pub fn telegram_disconnect(bridge: Shared<'_>, project: String) -> Result<(), String> {
    bridge.disconnect(&project);
    Ok(())
}

#[tauri::command]
pub async fn telegram_remove_webhook(bridge: Shared<'_>, project: String) -> Result<(), String> {
    blocking(&bridge, move |bridge| bridge.remove_webhook(&project)).await
}

#[tauri::command]
pub fn telegram_link_code(
    bridge: Shared<'_>,
    project: String,
    session_id: String,
) -> Result<LinkCode, String> {
    bridge.link_code(&project, &session_id)
}

#[tauri::command]
pub fn telegram_session_topic(
    bridge: Shared<'_>,
    project: String,
    session_id: String,
) -> Option<i64> {
    bridge.session_topic(&project, &session_id)
}

/// The window that posts a question owns it: the answer event goes back to that window.
#[tauri::command]
pub fn telegram_forward_question(
    window: WebviewWindow,
    bridge: Shared<'_>,
    payload: ForwardPayload,
) -> Result<(), String> {
    if !crate::window::is_workspace_window(window.label()) {
        return Err("Questions can only be forwarded from a workspace window".to_string());
    }
    bridge.forward(window.label(), payload)
}

/// `status` is the line shown on the Telegram message once the desktop settled the question.
#[tauri::command]
pub fn telegram_question_closed(
    bridge: Shared<'_>,
    session_id: String,
    request_id: u64,
    status: String,
) -> Result<(), String> {
    if !matches!(
        status.as_str(),
        "Answered on desktop" | "Skipped" | "Cancelled"
    ) {
        return Err("Unknown question status".to_string());
    }
    bridge.question_closed(&session_id, request_id, &status);
    Ok(())
}

/// The window's verdict on a `monocode-telegram-answer` event.
#[tauri::command]
pub fn telegram_answer_result(bridge: Shared<'_>, key: String, ok: bool) -> Result<(), String> {
    bridge.answer_result(&key, ok);
    Ok(())
}
