//! Telegram bridge runtime: one long-polling thread per bot token, one sender thread that
//! drains the rate-limited queue, and the operations the Tauri commands call.
//!
//! All shared state sits behind one mutex (`State`). No lock is held across an HTTP call,
//! an event emit or a sleep. The bot token never appears in a log line or a problem string.

use super::api::{BotApi, TgError};
use super::config::{self, CodePurpose, Codes, ProjectConfig, TelegramConfig};
use super::pending::{key_of, Effect, PendingPrompt, Registry};
use super::queue::{Job, SendQueue};
use super::render::{keyboard, question_html, status_html, ForwardPayload};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const API_BASE: &str = "https://api.telegram.org";
/// Event emitted to the window that owns a session when someone answers in Telegram.
pub const ANSWER_EVENT: &str = "monocode-telegram-answer";

const POLL_TIMEOUT_S: u64 = 50;
/// How long a paused poller (409) waits before it tries again.
const PAUSE_S: u64 = 60;
const MAX_BACKOFF_S: u64 = 60;
/// A reply handed to a window must be confirmed within this time, or it counts as failed.
const DELIVERY_TIMEOUT_MS: u64 = 30_000;
const TOPIC_NAME_MAX_CHARS: usize = 128;
const PROBLEM_MAX_CHARS: usize = 200;
/// Longest wait of the sender thread when nothing is scheduled.
const IDLE_WAIT: Duration = Duration::from_secs(5);

pub const PROBLEM_TOKEN_REJECTED: &str = "Bot token was rejected";
const PROBLEM_OTHER_APP: &str = "Another app is reading this bot";
const PROBLEM_WEBHOOK: &str = "This bot has a webhook";
const PROBLEM_NOT_FORUM: &str = "Topics are not enabled in this group";
const PROBLEM_NO_RIGHT: &str = "The bot needs the Manage topics admin right";
const PROBLEM_NO_TOPICS: &str = "Could not create a topic, so questions go to the General chat";

// ---------------------------------------------------------------------------------------
// Command DTOs. None of them can carry the token.

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TelegramStatus {
    pub configured: bool,
    pub enabled: bool,
    pub bot_username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_forum: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_topics: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectLink {
    pub code: String,
    pub url: String,
    pub fallback: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkCode {
    pub code: String,
    pub command: String,
}

// ---------------------------------------------------------------------------------------
// Pure helpers.

/// Builds the status the UI sees. The only way a project config leaves this module.
pub fn status_of(project: Option<&ProjectConfig>) -> TelegramStatus {
    let Some(p) = project else {
        return TelegramStatus::default();
    };
    TelegramStatus {
        configured: !p.token.is_empty(),
        enabled: p.enabled,
        bot_username: p.bot_username.clone(),
        chat_id: p.chat_id,
        chat_title: p.chat_title.clone(),
        is_forum: p.is_forum,
        can_manage_topics: p.can_manage_topics,
        problem: p.problem.clone(),
    }
}

/// Telegram's topic name limit is 128 characters, not bytes.
pub fn topic_name(title: &str) -> String {
    let name: String = title.trim().chars().take(TOPIC_NAME_MAX_CHARS).collect();
    let name = name.trim_end();
    if name.is_empty() {
        "Session".to_string()
    } else {
        name.to_string()
    }
}

pub fn connect_url(bot: &str, code: &str) -> String {
    format!("https://t.me/{bot}?startgroup={code}&admin=manage_topics")
}

pub fn connect_fallback(bot: &str, code: &str) -> String {
    format!("/connect@{bot} {code}")
}

pub fn link_command(bot: &str, code: &str) -> String {
    format!("/link@{bot} {code}")
}

/// `<digits>:<letters, digits, _ or ->`. The token becomes part of a URL, so anything else
/// is refused before it is used.
fn token_looks_valid(token: &str) -> bool {
    let Some((id, secret)) = token.split_once(':') else {
        return false;
    };
    !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && !secret.is_empty()
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Where polling continues after `updates`: one past the highest `update_id`.
fn next_offset(current: i64, updates: &[Value]) -> i64 {
    updates
        .iter()
        .filter_map(|update| update["update_id"].as_i64())
        .map(|id| id + 1)
        .fold(current, i64::max)
}

/// Wait after the `failures`-th failure in a row: 1, 2, 4 ... capped at 60 s.
fn backoff_secs(failures: u32) -> u64 {
    (1u64 << failures.saturating_sub(1).min(6)).min(MAX_BACKOFF_S)
}

#[derive(Debug, PartialEq)]
enum PollAction {
    /// The token is dead; polling ends.
    Stop,
    /// Another reader or a webhook owns the updates; wait and try again.
    Pause(&'static str),
    /// Telegram said how long to wait.
    Sleep(u64),
    /// Anything else (including a proxy page that parsed as Permanent) is retried.
    Backoff,
}

fn poll_action(error: &TgError) -> PollAction {
    match error {
        TgError::Unauthorized => PollAction::Stop,
        TgError::Conflict { webhook: true } => PollAction::Pause(PROBLEM_WEBHOOK),
        TgError::Conflict { webhook: false } => PollAction::Pause(PROBLEM_OTHER_APP),
        TgError::RetryAfter(seconds) => PollAction::Sleep(*seconds),
        _ => PollAction::Backoff,
    }
}

/// A friendly message for a failed `getMe`. Never echoes the request.
fn describe_token_error(error: &TgError) -> String {
    match error {
        TgError::Unauthorized | TgError::Permanent(_) => PROBLEM_TOKEN_REJECTED.to_string(),
        TgError::Network(_) => "Could not reach Telegram".to_string(),
        TgError::RetryAfter(_) => "Telegram is busy, try again in a moment".to_string(),
        _ => "Telegram sent an unexpected answer".to_string(),
    }
}

fn describe_error(error: &TgError) -> String {
    match error {
        TgError::Unauthorized => PROBLEM_TOKEN_REJECTED.to_string(),
        TgError::Network(_) => "Could not reach Telegram".to_string(),
        TgError::Permanent(text) => truncate(text),
        _ => "Telegram sent an unexpected answer".to_string(),
    }
}

fn truncate(text: &str) -> String {
    text.chars().take(PROBLEM_MAX_CHARS).collect()
}

/// Problem line for a message Telegram refused for good.
fn refused_problem(description: &str) -> String {
    if description.to_lowercase().contains("too long") {
        return "A question was too long for Telegram and was not sent".to_string();
    }
    truncate(&format!("Telegram refused a message: {description}"))
}

/// A project keeps its group and topics only when the same bot comes back.
fn apply_token(
    existing: Option<ProjectConfig>,
    token: &str,
    bot_id: i64,
    username: &str,
) -> ProjectConfig {
    let mut project = match existing {
        Some(project) if project.bot_id == bot_id && bot_id != 0 => project,
        _ => ProjectConfig::default(),
    };
    project.token = token.to_string();
    project.bot_id = bot_id;
    project.bot_username = username.to_string();
    project.problem = None;
    project
}

/// (topics enabled, bot may manage topics, problem to show).
fn topic_rights(chat: &Value, member: &Value) -> (bool, bool, Option<&'static str>) {
    let is_forum = chat["is_forum"].as_bool().unwrap_or(false);
    let can_manage = match member["status"].as_str() {
        Some("creator") => true,
        Some("administrator") => member["can_manage_topics"].as_bool().unwrap_or(false),
        _ => false,
    };
    let problem = if !is_forum {
        Some(PROBLEM_NOT_FORUM)
    } else if !can_manage {
        Some(PROBLEM_NO_RIGHT)
    } else {
        None
    };
    (is_forum, can_manage, problem)
}

fn is_rights_problem(problem: &str) -> bool {
    problem == PROBLEM_NOT_FORUM || problem == PROBLEM_NO_RIGHT
}

fn connected_text(problem: Option<&str>) -> String {
    let mut text = "Connected to Monoptah. Questions from this project's sessions will be \
                    posted here, one topic per session."
        .to_string();
    if let Some(problem) = problem {
        text.push_str(&format!("\n\nNote: {problem}."));
    }
    text
}

/// Tokens that need a polling thread: every configured project that is enabled, still
/// waiting for a group, or has a connect code outstanding.
fn desired_tokens(cfg: &TelegramConfig, awaiting: &BTreeSet<String>) -> BTreeSet<String> {
    cfg.projects
        .iter()
        .filter(|(name, p)| {
            !p.token.is_empty() && (p.enabled || p.chat_id.is_none() || awaiting.contains(*name))
        })
        .map(|(_, p)| p.token.clone())
        .collect()
}

/// Chats an update from `token` may act on.
fn configured_chats(cfg: &TelegramConfig, token: &str) -> BTreeSet<i64> {
    cfg.projects
        .values()
        .filter(|p| p.token == token)
        .filter_map(|p| p.chat_id)
        .collect()
}

// ---------------------------------------------------------------------------------------
// Runtime.

/// Where answer events go. The app implements it with Tauri windows; tests record calls.
pub trait EventSink: Send + Sync {
    /// True when the event was handed to an existing window.
    fn emit_to(&self, window: &str, event: &str, payload: Value) -> bool;
}

struct Poller {
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    cfg: TelegramConfig,
    registry: Registry,
    codes: Codes,
    queue: SendQueue,
    /// token -> thread. A thread that ended because the token was rejected stays here so it
    /// is not restarted until the user saves a token again.
    pollers: BTreeMap<String, Poller>,
    /// Projects that issued a connect code and wait for the bot to be added to a group.
    awaiting_connect: BTreeSet<String>,
    /// (project, session) pairs with a `CreateTopic` queued, so one topic is made per session.
    topics_pending: BTreeSet<(String, String)>,
    /// key -> deadline (bridge clock, ms) of replies handed to a window.
    deliveries: BTreeMap<String, u64>,
    /// chat -> consecutive network failures.
    failures: BTreeMap<i64, u32>,
}

pub struct Bridge {
    state: Mutex<State>,
    wake: Condvar,
    base: String,
    path: PathBuf,
    sink: Arc<dyn EventSink>,
    epoch: Instant,
    /// Test clock: added to the real elapsed time so rate-limit waits need no sleeping.
    #[cfg(test)]
    skew_ms: std::sync::atomic::AtomicU64,
}

fn sleep_unless_stopped(stop: &AtomicBool, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
    }
}

impl Bridge {
    pub fn new(base: &str, path: PathBuf, sink: Arc<dyn EventSink>) -> Arc<Self> {
        let state = State {
            cfg: config::load(&path),
            ..State::default()
        };
        Arc::new(Self {
            state: Mutex::new(state),
            wake: Condvar::new(),
            base: base.trim_end_matches('/').to_string(),
            path,
            sink,
            epoch: Instant::now(),
            #[cfg(test)]
            skew_ms: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Starts the sender thread and the pollers for the saved config.
    pub fn start(self: &Arc<Self>) {
        let bridge = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("telegram-sender".into())
            .spawn(move || loop {
                bridge.pump(true);
            });
        if spawned.is_err() {
            eprintln!("monoptah: telegram sender thread could not start");
        }
        self.sync_pollers();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn now_ms(&self) -> u64 {
        let elapsed = self.epoch.elapsed().as_millis() as u64;
        #[cfg(test)]
        let elapsed = elapsed + self.skew_ms.load(Ordering::Relaxed);
        elapsed
    }

    fn persist(&self, state: &State) {
        if config::save(&self.path, &state.cfg).is_err() {
            eprintln!("monoptah: telegram config could not be saved");
        }
    }

    fn api(&self, token: &str) -> BotApi {
        BotApi::new(&self.base, token)
    }

    fn api_for_project(&self, project: &str) -> Option<BotApi> {
        let state = self.lock();
        let token = &state.cfg.projects.get(project)?.token;
        (!token.is_empty()).then(|| self.api(token))
    }

    /// Any project connected to `chat` knows the token to use for it.
    fn api_for_chat(&self, chat: i64) -> Option<BotApi> {
        let state = self.lock();
        state
            .cfg
            .projects
            .values()
            .find(|p| p.chat_id == Some(chat) && !p.token.is_empty())
            .map(|p| self.api(&p.token))
    }

    // ----- problems -----

    fn set_problem(&self, state: &mut State, project: &str, text: &str) {
        let Some(p) = state.cfg.projects.get_mut(project) else {
            return;
        };
        p.problem = Some(truncate(&text.replace(&p.token, "<token>")));
        self.persist(state);
    }

    fn set_problem_for_chat(&self, chat: i64, text: &str) {
        let mut state = self.lock();
        let names: Vec<String> = state
            .cfg
            .projects
            .iter()
            .filter(|(_, p)| p.chat_id == Some(chat))
            .map(|(name, _)| name.clone())
            .collect();
        for name in names {
            self.set_problem(&mut state, &name, text);
        }
    }

    fn set_problem_for_token(&self, token: &str, text: &str) {
        let mut state = self.lock();
        let names: Vec<String> = state
            .cfg
            .projects
            .iter()
            .filter(|(_, p)| p.token == token)
            .map(|(name, _)| name.clone())
            .collect();
        for name in names {
            self.set_problem(&mut state, &name, text);
        }
    }

    /// Clears the 409 problems once getUpdates works again.
    fn clear_poll_problems(&self, token: &str) {
        let mut state = self.lock();
        let mut changed = false;
        for p in state.cfg.projects.values_mut().filter(|p| p.token == token) {
            if matches!(
                p.problem.as_deref(),
                Some(PROBLEM_OTHER_APP | PROBLEM_WEBHOOK)
            ) {
                p.problem = None;
                changed = true;
            }
        }
        if changed {
            self.persist(&state);
        }
    }

    // ----- pollers -----

    /// Starts a polling thread for every wanted token that has none and stops the others.
    pub fn sync_pollers(self: &Arc<Self>) {
        let mut state = self.lock();
        let desired = desired_tokens(&state.cfg, &state.awaiting_connect);
        state.pollers.retain(|token, poller| {
            let keep = desired.contains(token);
            if !keep {
                poller.stop.store(true, Ordering::Relaxed);
            }
            keep
        });
        for token in desired {
            if state.pollers.contains_key(&token) {
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            let bridge = Arc::clone(self);
            let (thread_token, thread_stop) = (token.clone(), Arc::clone(&stop));
            let spawned = std::thread::Builder::new()
                .name("telegram-poller".into())
                .spawn(move || bridge.run_poller(thread_token, thread_stop));
            if spawned.is_ok() {
                state.pollers.insert(token, Poller { stop });
            } else {
                eprintln!("monoptah: telegram poller thread could not start");
            }
        }
    }

    /// Stops the thread for `token` (a rejected one included) and starts a fresh one.
    fn restart_poller(self: &Arc<Self>, token: &str) {
        if let Some(poller) = self.lock().pollers.remove(token) {
            poller.stop.store(true, Ordering::Relaxed);
        }
        self.sync_pollers();
    }

    fn run_poller(self: Arc<Self>, token: String, stop: Arc<AtomicBool>) {
        let api = self.api(&token);
        let mut offset: Option<i64> = None;
        let mut failures = 0u32;
        while !stop.load(Ordering::Relaxed) {
            // The first call only learns where the backlog ends, so old updates are skipped.
            let result = match offset {
                None => api.get_updates(-1, 0),
                Some(next) => api.get_updates(next, POLL_TIMEOUT_S),
            };
            // A stopped poller may get a 409 from the thread that replaced it; ignore it.
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match result {
                Ok(updates) => {
                    failures = 0;
                    self.clear_poll_problems(&token);
                    match offset {
                        None => {
                            offset = Some(next_offset(0, &updates));
                            self.refresh_token_chats(&api, &token);
                        }
                        Some(mut next) => {
                            for update in &updates {
                                next = next_offset(next, std::slice::from_ref(update));
                                if stop.load(Ordering::Relaxed) {
                                    return;
                                }
                                self.process_update(&api, &token, update);
                            }
                            offset = Some(next);
                        }
                    }
                }
                Err(error) => match poll_action(&error) {
                    PollAction::Stop => {
                        self.set_problem_for_token(&token, PROBLEM_TOKEN_REJECTED);
                        return;
                    }
                    PollAction::Pause(problem) => {
                        self.set_problem_for_token(&token, problem);
                        sleep_unless_stopped(&stop, PAUSE_S);
                    }
                    PollAction::Sleep(seconds) => sleep_unless_stopped(&stop, seconds),
                    PollAction::Backoff => {
                        failures += 1;
                        sleep_unless_stopped(&stop, backoff_secs(failures));
                    }
                },
            }
        }
    }

    // ----- updates from Telegram -----

    fn process_update(&self, api: &BotApi, token: &str, update: &Value) {
        let now = self.now_ms();
        let effects = {
            let mut state = self.lock();
            let chats = configured_chats(&state.cfg, token);
            let State {
                registry, codes, ..
            } = &mut *state;
            registry.handle_update(update, &chats, codes, now)
        };
        let update_chat = update.pointer("/message/chat/id").and_then(Value::as_i64);
        let mut rest = Vec::new();
        for effect in effects {
            match effect {
                // Answered right away and outside the queue: Telegram drops late answers.
                Effect::AnswerCallback { id, text } => {
                    let _ = api.answer_callback_query(&id, &text);
                }
                Effect::Connected {
                    project,
                    chat,
                    title,
                } => self.on_connected(api, token, &project, chat, &title),
                Effect::Linked {
                    project,
                    session_id,
                    thread,
                } => self.on_linked(token, &project, &session_id, thread, update_chat),
                other => rest.push(other),
            }
        }
        self.apply_effects(rest);
    }

    /// A Connect code was used in `chat`. It must have been issued by a project of the bot
    /// that received it, otherwise the code is wasted and nothing changes.
    fn on_connected(&self, api: &BotApi, token: &str, project: &str, chat: i64, title: &str) {
        let bot_id = {
            let mut state = self.lock();
            let Some(p) = state.cfg.projects.get_mut(project) else {
                return;
            };
            if p.token != token {
                return;
            }
            if p.chat_id != Some(chat) {
                p.topics.clear();
            }
            p.chat_id = Some(chat);
            p.chat_title = Some(title.to_string());
            p.is_forum = None;
            p.can_manage_topics = None;
            p.problem = None;
            let bot_id = p.bot_id;
            state.awaiting_connect.remove(project);
            self.persist(&state);
            bot_id
        };
        let problem = self.refresh_chat_info(api, project, chat, bot_id);
        self.push_job(
            chat,
            Job::Notice {
                thread: None,
                html: connected_text(problem.as_deref()),
            },
        );
    }

    /// A Link code was used inside a topic. The code's project must own the chat the message
    /// came from; a code from another project is refused.
    fn on_linked(
        &self,
        token: &str,
        project: &str,
        session_id: &str,
        thread: i64,
        update_chat: Option<i64>,
    ) {
        let Some(chat) = update_chat else {
            return;
        };
        let allowed = {
            let mut state = self.lock();
            match state.cfg.projects.get_mut(project) {
                Some(p) if p.token == token && p.chat_id == Some(chat) => {
                    p.topics.insert(session_id.to_string(), thread);
                    self.persist(&state);
                    true
                }
                _ => false,
            }
        };
        let html = if allowed {
            "Linked. Questions from this session will be posted in this topic."
        } else {
            "That code belongs to another project or group."
        };
        self.push_job(
            chat,
            Job::Notice {
                thread: Some(thread),
                html: html.to_string(),
            },
        );
    }

    /// Records whether Topics are on and whether the bot may manage them. Returns the
    /// problem to show, if any. A failed lookup changes nothing.
    fn refresh_chat_info(
        &self,
        api: &BotApi,
        project: &str,
        chat: i64,
        bot_id: i64,
    ) -> Option<String> {
        if bot_id == 0 {
            return None;
        }
        let chat_info = api.get_chat(chat).ok()?;
        let member = api.get_chat_member(chat, bot_id).ok()?;
        let (is_forum, can_manage, problem) = topic_rights(&chat_info, &member);
        let mut state = self.lock();
        let p = state.cfg.projects.get_mut(project)?;
        p.is_forum = Some(is_forum);
        p.can_manage_topics = Some(can_manage);
        match problem {
            Some(text) => p.problem = Some(text.to_string()),
            None if p.problem.as_deref().is_some_and(is_rights_problem) => p.problem = None,
            None => {}
        }
        let shown = p.problem.clone();
        self.persist(&state);
        shown
    }

    /// After a start, re-checks the group of every project of this bot.
    fn refresh_token_chats(&self, api: &BotApi, token: &str) {
        let targets: Vec<(String, i64, i64)> = self
            .lock()
            .cfg
            .projects
            .iter()
            .filter(|(_, p)| p.token == token)
            .filter_map(|(name, p)| Some((name.clone(), p.chat_id?, p.bot_id)))
            .collect();
        for (project, chat, bot_id) in targets {
            self.refresh_chat_info(api, &project, chat, bot_id);
        }
    }

    // ----- effects and deliveries -----

    fn push_job(&self, chat: i64, job: Job) {
        self.lock().queue.push(chat, job);
        self.wake.notify_all();
    }

    /// Queues the edits and replies a registry call asked for and hands finished prompts to
    /// their window. Callback answers, Connect and Link are handled by `process_update`.
    fn apply_effects(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Edit {
                    chat,
                    message_id,
                    html,
                    keyboard,
                } => self.push_job(
                    chat,
                    Job::EditStatus {
                        chat,
                        message_id,
                        html,
                        keyboard,
                    },
                ),
                Effect::Reply { chat, thread, html } => {
                    self.push_job(chat, Job::Notice { thread, html })
                }
                Effect::Deliver {
                    window,
                    session_id,
                    request_id,
                    reply,
                    by,
                } => self.deliver(&window, &session_id, request_id, reply, &by),
                Effect::AnswerCallback { .. }
                | Effect::Connected { .. }
                | Effect::Linked { .. } => {}
            }
        }
    }

    /// Hands a finished prompt to its window. `Registry::delivered` is always called
    /// eventually: at once when the window is gone, on the window's result, or on timeout.
    fn deliver(&self, window: &str, session_id: &str, request_id: u64, reply: Value, by: &str) {
        let key = key_of(session_id, request_id);
        let deadline = self.now_ms() + DELIVERY_TIMEOUT_MS;
        self.lock().deliveries.insert(key.clone(), deadline);
        let payload = json!({
            "key": key,
            "sessionId": session_id,
            "requestId": request_id,
            "reply": reply,
            "by": by,
        });
        if self.sink.emit_to(window, ANSWER_EVENT, payload) {
            self.wake.notify_all();
        } else {
            self.answer_result(&key, false);
        }
    }

    /// The window's verdict on a delivered reply.
    pub fn answer_result(&self, key: &str, ok: bool) {
        let effects = {
            let mut state = self.lock();
            state.deliveries.remove(key);
            state.registry.delivered(key, ok)
        };
        self.apply_effects(effects);
    }

    /// Treats every reply not confirmed in time as failed, so no prompt stays behind.
    fn expire_deliveries(&self, now_ms: u64) {
        let expired: Vec<String> = {
            let state = self.lock();
            state
                .deliveries
                .iter()
                .filter(|(_, deadline)| **deadline <= now_ms)
                .map(|(key, _)| key.clone())
                .collect()
        };
        for key in expired {
            self.answer_result(&key, false);
        }
    }

    // ----- sender -----

    /// Runs at most one queued job. With `block`, waits until one is due and always returns
    /// true after running it. Without, returns false when nothing is due.
    fn pump(&self, block: bool) -> bool {
        loop {
            let now = self.now_ms();
            self.expire_deliveries(now);
            let mut state = self.lock();
            if let Some((chat, job)) = state.queue.next_ready(now) {
                drop(state);
                self.run_job(chat, job);
                return true;
            }
            if !block {
                return false;
            }
            let due = [
                state.queue.next_wake(now),
                state.deliveries.values().min().copied(),
            ]
            .into_iter()
            .flatten()
            .min();
            let wait = due.map_or(IDLE_WAIT, |at| {
                Duration::from_millis(at.saturating_sub(now).max(1))
            });
            drop(
                self.wake
                    .wait_timeout(state, wait)
                    .unwrap_or_else(|error| error.into_inner()),
            );
        }
    }

    fn run_job(&self, chat: i64, job: Job) {
        match job {
            Job::CreateTopic { .. } => self.run_create_topic(chat, job),
            Job::SendQuestion { .. } => self.run_send_question(chat, job),
            Job::EditStatus { .. } => self.run_edit(chat, job),
            Job::Notice { .. } => self.run_notice(chat, job),
        }
    }

    fn run_create_topic(&self, chat: i64, job: Job) {
        let Job::CreateTopic {
            project,
            session_id,
            name,
        } = &job
        else {
            return;
        };
        let Some(api) = self.api_for_project(project) else {
            self.release_job(&job);
            return;
        };
        match api.create_forum_topic(chat, &topic_name(name)) {
            Ok(thread) => {
                let mut state = self.lock();
                state
                    .topics_pending
                    .remove(&(project.clone(), session_id.clone()));
                state.failures.remove(&chat);
                if let Some(p) = state.cfg.projects.get_mut(project) {
                    p.topics.insert(session_id.clone(), thread);
                    if p.problem.as_deref() == Some(PROBLEM_NO_TOPICS) {
                        p.problem = None;
                    }
                }
                self.persist(&state);
            }
            // No rights, or the group has no Topics: the question goes to General instead.
            Err(TgError::Permanent(_)) => {
                let mut state = self.lock();
                state
                    .topics_pending
                    .remove(&(project.clone(), session_id.clone()));
                self.set_problem(&mut state, project, PROBLEM_NO_TOPICS);
            }
            Err(error) => self.handle_send_error(chat, job, error),
        }
    }

    fn run_send_question(&self, chat: i64, job: Job) {
        let Job::SendQuestion { key } = &job else {
            return;
        };
        // Everything needed is copied out, so no lock is held while sending.
        let plan = {
            let state = self.lock();
            state.registry.prompt(key).and_then(|prompt| {
                let project = state.cfg.projects.get(&prompt.payload.project)?;
                let items: Vec<(usize, String, Value)> = (0..prompt.payload.questions.len())
                    .filter(|i| prompt.messages[*i].is_none() && !prompt.done[*i])
                    .map(|i| {
                        let selected = &prompt.selected[i];
                        (
                            i,
                            question_html(&prompt.payload, i, selected),
                            keyboard(&prompt.tokens[i], &prompt.payload.questions[i], selected),
                        )
                    })
                    .collect();
                Some((
                    prompt.payload.clone(),
                    project.topics.get(&prompt.payload.session_id).copied(),
                    self.api(&project.token),
                    items,
                ))
            })
        };
        let Some((payload, thread, api, items)) = plan else {
            // The prompt was closed or its project removed while the job waited.
            return;
        };
        for (index, html, markup) in items {
            match api.send_message(chat, thread, &html, Some(markup)) {
                Ok(message_id) => {
                    let still_waiting = {
                        let mut state = self.lock();
                        state.registry.set_message(key, index, message_id);
                        state.registry.prompt(key).is_some()
                    };
                    if !still_waiting {
                        // Closed while the message was in flight: do not leave live buttons.
                        let html =
                            status_html(&payload, index, "This question is no longer waiting");
                        self.push_job(
                            chat,
                            Job::EditStatus {
                                chat,
                                message_id,
                                html,
                                keyboard: None,
                            },
                        );
                        return;
                    }
                }
                Err(TgError::ThreadNotFound) if thread.is_some() => {
                    self.recover_topic(chat, job.clone(), &payload, thread);
                    return;
                }
                Err(error) => {
                    self.handle_send_error(chat, job, error);
                    return;
                }
            }
        }
        self.sent_ok(chat);
    }

    /// The bound topic was deleted: forget it, make a new one first, then retry the question.
    fn recover_topic(&self, chat: i64, job: Job, payload: &ForwardPayload, used: Option<i64>) {
        let mut state = self.lock();
        let slot = (payload.project.clone(), payload.session_id.clone());
        let mut dropped = false;
        if let Some(p) = state.cfg.projects.get_mut(&payload.project) {
            if p.topics.get(&payload.session_id).copied() == used {
                p.topics.remove(&payload.session_id);
                dropped = true;
            }
        }
        if dropped {
            self.persist(&state);
        }
        let needs_topic = !state
            .cfg
            .projects
            .get(&payload.project)
            .is_some_and(|p| p.topics.contains_key(&payload.session_id));
        state.queue.requeue_front(chat, job);
        if needs_topic && state.topics_pending.insert(slot) {
            state.queue.requeue_front(
                chat,
                Job::CreateTopic {
                    project: payload.project.clone(),
                    session_id: payload.session_id.clone(),
                    name: payload.topic_title.clone(),
                },
            );
        }
    }

    fn run_edit(&self, chat: i64, job: Job) {
        let Job::EditStatus {
            message_id,
            html,
            keyboard,
            ..
        } = &job
        else {
            return;
        };
        let Some(api) = self.api_for_chat(chat) else {
            return;
        };
        match api.edit_message(chat, *message_id, html, keyboard.clone()) {
            Ok(()) | Err(TgError::NotModified) => self.sent_ok(chat),
            Err(error) => self.handle_send_error(chat, job, error),
        }
    }

    fn run_notice(&self, chat: i64, job: Job) {
        let Job::Notice { thread, html } = &job else {
            return;
        };
        let Some(api) = self.api_for_chat(chat) else {
            return;
        };
        match api.send_message(chat, *thread, html, None) {
            Ok(_) => self.sent_ok(chat),
            // A reply into a deleted topic is not worth a problem line.
            Err(TgError::ThreadNotFound) => {}
            Err(error) => self.handle_send_error(chat, job, error),
        }
    }

    fn sent_ok(&self, chat: i64) {
        self.lock().failures.remove(&chat);
    }

    /// Frees the bookkeeping of a job that will not run again.
    fn release_job(&self, job: &Job) {
        if let Job::CreateTopic {
            project,
            session_id,
            ..
        } = job
        {
            self.lock()
                .topics_pending
                .remove(&(project.clone(), session_id.clone()));
        }
    }

    /// Common outcome of a failed send. `chat` is the queue's chat, which is also the chat
    /// the request used.
    fn handle_send_error(&self, chat: i64, job: Job, error: TgError) {
        let now = self.now_ms();
        match error {
            TgError::RetryAfter(seconds) => {
                let mut state = self.lock();
                state.queue.pause(chat, now + seconds.saturating_mul(1000));
                state.queue.requeue_front(chat, job);
            }
            TgError::Network(_) => {
                let mut state = self.lock();
                let failures = state.failures.entry(chat).or_insert(0);
                *failures += 1;
                let wait = backoff_secs(*failures);
                state.queue.pause(chat, now + wait * 1000);
                state.queue.requeue_front(chat, job);
            }
            TgError::Migrated(new_chat) if new_chat != chat => {
                self.migrate_chat(chat, new_chat);
                let job = match job {
                    Job::EditStatus {
                        message_id,
                        html,
                        keyboard,
                        ..
                    } => Job::EditStatus {
                        chat: new_chat,
                        message_id,
                        html,
                        keyboard,
                    },
                    other => other,
                };
                self.lock().queue.requeue_front(new_chat, job);
            }
            TgError::NotModified => self.sent_ok(chat),
            TgError::Unauthorized => {
                self.set_problem_for_chat(chat, PROBLEM_TOKEN_REJECTED);
                self.drop_job(job);
            }
            other => {
                let description = match &other {
                    TgError::Permanent(text) => text.clone(),
                    _ => describe_error(&other),
                };
                self.set_problem_for_chat(chat, &refused_problem(&description));
                self.drop_job(job);
            }
        }
        self.wake.notify_all();
    }

    /// Gives up on a job. A question that cannot be sent is closed so it does not linger.
    fn drop_job(&self, job: Job) {
        self.release_job(&job);
        if let Job::SendQuestion { key } = &job {
            let effects = {
                let mut state = self.lock();
                let ids = state
                    .registry
                    .prompt(key)
                    .map(|p| (p.payload.session_id.clone(), p.payload.request_id));
                match ids {
                    Some((session_id, request_id)) => state.registry.close(
                        &session_id,
                        request_id,
                        "Could not be sent to Telegram",
                    ),
                    None => Vec::new(),
                }
            };
            self.apply_effects(effects);
        }
    }

    /// The group became a supergroup: config, prompts and queued work follow it.
    fn migrate_chat(&self, old: i64, new: i64) {
        let mut state = self.lock();
        for p in state.cfg.projects.values_mut() {
            if p.chat_id == Some(old) {
                p.chat_id = Some(new);
            }
        }
        state.registry.migrate_chat(old, new);
        state.queue.rename_chat(old, new);
        if let Some(count) = state.failures.remove(&old) {
            state.failures.insert(new, count);
        }
        self.persist(&state);
    }

    // ----- operations behind the commands -----

    pub fn status(&self, project: &str) -> TelegramStatus {
        status_of(self.lock().cfg.projects.get(project))
    }

    /// Checks the token with `getMe`, stores it and restarts polling for it. Blocking.
    pub fn set_token(
        self: &Arc<Self>,
        project: &str,
        token: &str,
    ) -> Result<TelegramStatus, String> {
        let token = token.trim();
        if !token_looks_valid(token) {
            return Err(PROBLEM_TOKEN_REJECTED.to_string());
        }
        let me = self
            .api(token)
            .get_me()
            .map_err(|e| describe_token_error(&e))?;
        let bot_id = me["id"].as_i64().unwrap_or(0);
        let username = me["username"].as_str().unwrap_or("").to_string();
        if bot_id == 0 || username.is_empty() {
            return Err("Telegram did not describe this bot".to_string());
        }
        let status = {
            let mut state = self.lock();
            let existing = state.cfg.projects.remove(project);
            let updated = apply_token(existing, token, bot_id, &username);
            let status = status_of(Some(&updated));
            state.cfg.projects.insert(project.to_string(), updated);
            self.persist(&state);
            status
        };
        self.restart_poller(token);
        Ok(status)
    }

    pub fn connect_link(self: &Arc<Self>, project: &str) -> Result<ConnectLink, String> {
        let now = self.now_ms();
        let link = {
            let mut state = self.lock();
            let bot = match state.cfg.projects.get(project) {
                Some(p) if !p.token.is_empty() && !p.bot_username.is_empty() => {
                    p.bot_username.clone()
                }
                _ => return Err("Save a bot token first".to_string()),
            };
            let code = state.codes.issue(
                CodePurpose::Connect {
                    project: project.to_string(),
                },
                now,
            );
            state.awaiting_connect.insert(project.to_string());
            ConnectLink {
                url: connect_url(&bot, &code),
                fallback: connect_fallback(&bot, &code),
                code,
            }
        };
        self.sync_pollers();
        Ok(link)
    }

    pub fn set_enabled(
        self: &Arc<Self>,
        project: &str,
        enabled: bool,
    ) -> Result<TelegramStatus, String> {
        let (status, token) = {
            let mut state = self.lock();
            let p = state
                .cfg
                .projects
                .get_mut(project)
                .filter(|p| !p.token.is_empty())
                .ok_or("Save a bot token first")?;
            p.enabled = enabled;
            if enabled {
                p.problem = None;
            }
            let result = (status_of(Some(p)), p.token.clone());
            self.persist(&state);
            result
        };
        if enabled {
            self.restart_poller(&token);
        } else {
            self.sync_pollers();
        }
        Ok(status)
    }

    pub fn disconnect(self: &Arc<Self>, project: &str) {
        {
            let mut state = self.lock();
            state.cfg.projects.remove(project);
            state.awaiting_connect.remove(project);
            self.persist(&state);
        }
        self.sync_pollers();
    }

    /// Calls `deleteWebhook` for the project's bot and restarts its poller. Blocking.
    pub fn remove_webhook(self: &Arc<Self>, project: &str) -> Result<(), String> {
        let token = {
            let state = self.lock();
            state
                .cfg
                .projects
                .get(project)
                .map(|p| p.token.clone())
                .filter(|token| !token.is_empty())
                .ok_or("Save a bot token first")?
        };
        self.api(&token)
            .delete_webhook()
            .map_err(|e| describe_error(&e))?;
        self.clear_poll_problems(&token);
        self.restart_poller(&token);
        Ok(())
    }

    pub fn link_code(&self, project: &str, session_id: &str) -> Result<LinkCode, String> {
        let now = self.now_ms();
        let mut state = self.lock();
        let bot = match state.cfg.projects.get(project) {
            Some(p) if p.chat_id.is_some() && !p.bot_username.is_empty() => p.bot_username.clone(),
            _ => return Err("Connect a Telegram group first".to_string()),
        };
        let code = state.codes.issue(
            CodePurpose::Link {
                project: project.to_string(),
                session_id: session_id.to_string(),
            },
            now,
        );
        Ok(LinkCode {
            command: link_command(&bot, &code),
            code,
        })
    }

    pub fn session_topic(&self, project: &str, session_id: &str) -> Option<i64> {
        self.lock()
            .cfg
            .projects
            .get(project)?
            .topics
            .get(session_id)
            .copied()
    }

    /// Registers a question that stayed unanswered and queues it for Telegram.
    pub fn forward(&self, window: &str, payload: ForwardPayload) -> Result<(), String> {
        if payload.questions.is_empty() {
            return Err("The prompt has no questions".to_string());
        }
        let key = key_of(&payload.session_id, payload.request_id);
        {
            let mut state = self.lock();
            let project = state
                .cfg
                .projects
                .get(&payload.project)
                .filter(|p| !p.token.is_empty())
                .ok_or("Telegram is not set up for this project")?;
            if !project.enabled {
                return Err("Telegram is turned off for this project".to_string());
            }
            let chat = project
                .chat_id
                .ok_or("No Telegram group is connected for this project")?;
            // Forwarding the same prompt twice must not send it twice.
            if state.registry.prompt(&key).is_some() {
                return Ok(());
            }
            let has_topic = project.topics.contains_key(&payload.session_id);
            let slot = (payload.project.clone(), payload.session_id.clone());
            let create = Job::CreateTopic {
                project: payload.project.clone(),
                session_id: payload.session_id.clone(),
                name: payload.topic_title.clone(),
            };
            state
                .registry
                .insert(PendingPrompt::new(window.to_string(), payload, chat));
            if !has_topic && state.topics_pending.insert(slot) {
                state.queue.push(chat, create);
            }
            state.queue.push(chat, Job::SendQuestion { key });
        }
        self.wake.notify_all();
        Ok(())
    }

    /// The desktop answered, skipped or cancelled a forwarded question.
    pub fn question_closed(&self, session_id: &str, request_id: u64, status: &str) {
        let effects = self.lock().registry.close(session_id, request_id, status);
        self.apply_effects(effects);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::render::{OptionIn, QuestionIn};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    const TOKEN: &str = "123456:SECRET-token-value";
    const CHAT: i64 = -1001;

    // ----- pure helpers -----

    #[test]
    fn topic_name_trimmed_to_128() {
        let long = "\u{e9}".repeat(200);
        let name = topic_name(&long);
        assert_eq!(name.chars().count(), 128);
        assert!(name.len() > 128, "counted by characters, not bytes");
        let emoji = format!("  {}  ", "\u{1F680}".repeat(300));
        assert_eq!(topic_name(&emoji).chars().count(), 128);
        assert_eq!(topic_name("  Fix login  "), "Fix login");
        assert_eq!(topic_name("   "), "Session");
    }

    #[test]
    fn connect_link_url_format() {
        assert_eq!(
            connect_url("bot", "CODE"),
            "https://t.me/bot?startgroup=CODE&admin=manage_topics"
        );
        assert_eq!(connect_fallback("bot", "CODE"), "/connect@bot CODE");
        assert_eq!(link_command("bot", "CODE"), "/link@bot CODE");
    }

    #[test]
    fn status_never_contains_token() {
        let project = ProjectConfig {
            token: TOKEN.into(),
            bot_username: "my_bot".into(),
            bot_id: 123456,
            chat_id: Some(CHAT),
            chat_title: Some("Team".into()),
            is_forum: Some(true),
            can_manage_topics: Some(false),
            enabled: true,
            problem: Some("The bot needs the Manage topics admin right".into()),
            ..ProjectConfig::default()
        };
        let json = serde_json::to_string(&status_of(Some(&project))).unwrap();
        assert!(!json.contains("SECRET"), "{json}");
        assert!(!json.contains("123456:"), "{json}");
        assert!(!json.contains("token"), "{json}");
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["configured"], true);
        assert_eq!(value["botUsername"], "my_bot");
        assert_eq!(value["chatId"], CHAT);
        assert_eq!(value["canManageTopics"], false);
        assert_eq!(
            serde_json::to_value(status_of(None)).unwrap(),
            json!({ "configured": false, "enabled": false, "botUsername": "" })
        );
    }

    #[test]
    fn problems_cannot_carry_the_token() {
        let (bridge, _sink) = bridge_with("http://127.0.0.1:9");
        add_project(&bridge, "proj", true);
        {
            let mut state = bridge.lock();
            bridge.set_problem(&mut state, "proj", &format!("failed for {TOKEN}"));
        }
        let problem = bridge.status("proj").problem.unwrap();
        assert!(!problem.contains("SECRET"), "{problem}");
    }

    #[test]
    fn offsets_and_backoff() {
        assert_eq!(next_offset(0, &[]), 0);
        assert_eq!(
            next_offset(0, &[json!({"update_id": 5}), json!({"update_id": 9})]),
            10
        );
        assert_eq!(next_offset(20, &[json!({"update_id": 5})]), 20);
        let waits: Vec<u64> = (1..=8).map(backoff_secs).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60]);
    }

    #[test]
    fn poll_errors_map_to_actions() {
        assert_eq!(poll_action(&TgError::Unauthorized), PollAction::Stop);
        assert_eq!(
            poll_action(&TgError::Conflict { webhook: false }),
            PollAction::Pause("Another app is reading this bot")
        );
        assert_eq!(
            poll_action(&TgError::Conflict { webhook: true }),
            PollAction::Pause("This bot has a webhook")
        );
        // A proxy page that parsed as Permanent must not end polling.
        assert_eq!(
            poll_action(&TgError::Permanent("unreadable".into())),
            PollAction::Backoff
        );
        assert_eq!(
            poll_action(&TgError::Network("HTTP 502".into())),
            PollAction::Backoff
        );
        assert_eq!(poll_action(&TgError::RetryAfter(7)), PollAction::Sleep(7));
    }

    #[test]
    fn rejected_token_gets_a_friendly_error() {
        assert_eq!(
            describe_token_error(&TgError::Permanent("HTTP 404".into())),
            "Bot token was rejected"
        );
        assert_eq!(
            describe_token_error(&TgError::Unauthorized),
            "Bot token was rejected"
        );
        assert!(token_looks_valid("123456:AbC-d_9"));
        for bad in ["", "abc", "123:", ":abc", "12 3:abc", "123:ab/c", "123:a?b"] {
            assert!(!token_looks_valid(bad), "{bad}");
        }
    }

    #[test]
    fn new_bot_resets_group_and_topics() {
        let mut old = ProjectConfig {
            token: "1:old".into(),
            bot_id: 1,
            chat_id: Some(CHAT),
            enabled: true,
            problem: Some("x".into()),
            ..ProjectConfig::default()
        };
        old.topics.insert("s".into(), 5);
        let same = apply_token(Some(old.clone()), "1:new", 1, "bot");
        assert_eq!(same.chat_id, Some(CHAT));
        assert_eq!(same.topics.len(), 1);
        assert_eq!(same.token, "1:new");
        assert!(same.problem.is_none());
        let other = apply_token(Some(old), "2:new", 2, "other");
        assert_eq!(other.chat_id, None);
        assert!(other.topics.is_empty());
        assert!(!other.enabled);
    }

    #[test]
    fn topic_rights_cases() {
        let forum = json!({ "is_forum": true });
        let plain = json!({});
        let admin = json!({ "status": "administrator", "can_manage_topics": true });
        let weak = json!({ "status": "administrator" });
        let member = json!({ "status": "member" });
        assert_eq!(topic_rights(&forum, &admin), (true, true, None));
        assert_eq!(
            topic_rights(&forum, &weak),
            (true, false, Some(PROBLEM_NO_RIGHT))
        );
        assert_eq!(
            topic_rights(&forum, &member),
            (true, false, Some(PROBLEM_NO_RIGHT))
        );
        assert_eq!(
            topic_rights(&plain, &admin),
            (false, true, Some(PROBLEM_NOT_FORUM))
        );
    }

    #[test]
    fn desired_tokens_follow_enabled_and_pending_connect() {
        let mut cfg = TelegramConfig::default();
        let mk = |token: &str, chat: Option<i64>, enabled: bool| ProjectConfig {
            token: token.into(),
            chat_id: chat,
            enabled,
            ..ProjectConfig::default()
        };
        cfg.projects.insert("on".into(), mk("1:a", Some(1), true));
        cfg.projects.insert("off".into(), mk("2:b", Some(2), false));
        cfg.projects.insert("new".into(), mk("3:c", None, false));
        cfg.projects.insert("none".into(), mk("", None, true));
        let none = BTreeSet::new();
        let wanted: Vec<String> = desired_tokens(&cfg, &none).into_iter().collect();
        assert_eq!(wanted, ["1:a", "3:c"]);
        let awaiting: BTreeSet<String> = ["off".to_string()].into();
        assert!(desired_tokens(&cfg, &awaiting).contains("2:b"));
    }

    // ----- fixtures -----

    #[derive(Default)]
    struct FakeSink {
        open: Mutex<BTreeSet<String>>,
        events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for FakeSink {
        fn emit_to(&self, window: &str, event: &str, payload: Value) -> bool {
            if !self.open.lock().unwrap().contains(window) {
                return false;
            }
            assert_eq!(event, ANSWER_EVENT);
            self.events
                .lock()
                .unwrap()
                .push((window.to_string(), payload));
            true
        }
    }

    fn bridge_with(base: &str) -> (Arc<Bridge>, Arc<FakeSink>) {
        let dir =
            std::env::temp_dir().join(format!("monoptah-telegram-bridge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let sink = Arc::new(FakeSink::default());
        sink.open.lock().unwrap().insert("main".into());
        let bridge = Bridge::new(base, config::config_path(&dir), sink.clone());
        (bridge, sink)
    }

    fn add_project(bridge: &Arc<Bridge>, name: &str, enabled: bool) {
        let mut state = bridge.lock();
        state.cfg.projects.insert(
            name.into(),
            ProjectConfig {
                token: TOKEN.into(),
                bot_username: "my_bot".into(),
                bot_id: 123456,
                chat_id: Some(CHAT),
                enabled,
                ..ProjectConfig::default()
            },
        );
    }

    fn bind_topic(bridge: &Arc<Bridge>, project: &str, session: &str, thread: i64) {
        bridge
            .lock()
            .cfg
            .projects
            .get_mut(project)
            .unwrap()
            .topics
            .insert(session.into(), thread);
    }

    fn payload(session: &str, request_id: u64) -> ForwardPayload {
        ForwardPayload {
            project: "proj".into(),
            session_id: session.into(),
            request_id,
            topic_title: "Fix login".into(),
            header: "proj - Session".into(),
            context: "ctx".into(),
            questions: vec![QuestionIn {
                id: "q1".into(),
                header: None,
                prompt: "Which one?".into(),
                multi_select: false,
                allow_custom: false,
                options: vec![
                    OptionIn {
                        id: "a".into(),
                        label: "Alpha".into(),
                        description: None,
                    },
                    OptionIn {
                        id: "b".into(),
                        label: "Beta".into(),
                        description: None,
                    },
                ],
            }],
        }
    }

    struct Recorded {
        line: String,
        body: Value,
        at: Instant,
    }

    /// Serves the scripted responses in order, one connection each, and returns what it saw.
    fn serve_script(script: Vec<(u16, Value)>) -> (String, thread::JoinHandle<Vec<Recorded>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, body) in script {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                return seen;
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                let at = Instant::now();
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut raw = vec![0u8; length];
                reader.read_exact(&mut raw).unwrap();
                let body_text = body.to_string();
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_text}",
                    body_text.len()
                )
                .unwrap();
                seen.push(Recorded {
                    line,
                    body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
                    at,
                });
            }
            seen
        });
        (base, server)
    }

    fn ok(result: Value) -> (u16, Value) {
        (200, json!({ "ok": true, "result": result }))
    }

    fn fail(code: u16, description: &str, parameters: Value) -> (u16, Value) {
        (
            code,
            json!({ "ok": false, "error_code": code, "description": description, "parameters": parameters }),
        )
    }

    /// Runs queued jobs, moving the test clock forward when rate limits make them wait.
    fn drain(bridge: &Arc<Bridge>) {
        for _ in 0..40 {
            while bridge.pump(false) {}
            let now = bridge.now_ms();
            if bridge.lock().queue.next_wake(now).is_none() {
                return;
            }
            bridge.skew_ms.fetch_add(1_100, Ordering::Relaxed);
        }
        panic!("queue never drained");
    }

    fn method_of(recorded: &Recorded) -> String {
        recorded
            .line
            .split('/')
            .nth(2)
            .unwrap_or("")
            .split(' ')
            .next()
            .unwrap_or("")
            .to_string()
    }

    // ----- sender -----

    #[test]
    fn sender_retries_after_429() {
        let (base, server) = serve_script(vec![
            fail(
                429,
                "Too Many Requests: retry after 1",
                json!({ "retry_after": 1 }),
            ),
            ok(json!({ "message_id": 77 })),
        ]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        bind_topic(&bridge, "proj", "s1", 42);
        bridge.forward("main", payload("s1", 1)).unwrap();

        // Real clock on purpose: the retry has to wait out the 1 s from Telegram.
        let started = Instant::now();
        while bridge.lock().registry.prompt("s1#1").unwrap().messages[0].is_none() {
            bridge.pump(false);
            assert!(started.elapsed() < Duration::from_secs(5), "never retried");
            thread::sleep(Duration::from_millis(20));
        }
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].at.duration_since(seen[0].at) >= Duration::from_millis(1000));
        assert_eq!(seen[1].body["message_thread_id"], 42);
        assert_eq!(
            bridge.lock().registry.prompt("s1#1").unwrap().messages[0],
            Some(77)
        );
    }

    #[test]
    fn migrated_chat_updates_config() {
        let new_chat = -1002_i64;
        let (base, server) = serve_script(vec![
            fail(
                400,
                "Bad Request: group chat was upgraded to a supergroup chat",
                json!({ "migrate_to_chat_id": new_chat }),
            ),
            ok(json!({ "message_id": 5 })),
        ]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        bind_topic(&bridge, "proj", "s1", 42);
        bridge.forward("main", payload("s1", 1)).unwrap();
        drain(&bridge);

        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].body["chat_id"], CHAT);
        assert_eq!(seen[1].body["chat_id"], new_chat);
        let state = bridge.lock();
        assert_eq!(state.cfg.projects["proj"].chat_id, Some(new_chat));
        assert_eq!(state.registry.prompt("s1#1").unwrap().chat, new_chat);
        assert_eq!(state.registry.prompt("s1#1").unwrap().messages[0], Some(5));
        // Persisted too.
        assert_eq!(
            config::load(&bridge.path).projects["proj"].chat_id,
            Some(new_chat)
        );
    }

    #[test]
    fn thread_not_found_recreates_topic() {
        let (base, server) = serve_script(vec![
            fail(400, "Bad Request: message thread not found", json!({})),
            ok(json!({ "message_thread_id": 99 })),
            ok(json!({ "message_id": 7 })),
        ]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        bind_topic(&bridge, "proj", "s1", 42);
        bridge.forward("main", payload("s1", 1)).unwrap();
        drain(&bridge);

        let seen = server.join().unwrap();
        let methods: Vec<String> = seen.iter().map(method_of).collect();
        assert_eq!(methods.len(), 3, "{methods:?}");
        assert!(methods[0].ends_with("sendMessage"));
        assert!(methods[1].ends_with("createForumTopic"));
        assert!(methods[2].ends_with("sendMessage"));
        assert_eq!(seen[0].body["message_thread_id"], 42);
        assert_eq!(seen[1].body["name"], "Fix login");
        assert_eq!(seen[2].body["message_thread_id"], 99);
        assert_eq!(bridge.session_topic("proj", "s1"), Some(99));
        assert_eq!(
            bridge.lock().registry.prompt("s1#1").unwrap().messages[0],
            Some(7)
        );
    }

    #[test]
    fn creates_topic_with_trimmed_name_then_sends_question() {
        let (base, server) = serve_script(vec![
            ok(json!({ "message_thread_id": 11 })),
            ok(json!({ "message_id": 3 })),
        ]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        let mut p = payload("s1", 1);
        p.topic_title = "x".repeat(300);
        bridge.forward("main", p).unwrap();
        drain(&bridge);
        let seen = server.join().unwrap();
        assert!(method_of(&seen[0]).ends_with("createForumTopic"));
        assert_eq!(seen[0].body["name"].as_str().unwrap().chars().count(), 128);
        assert!(method_of(&seen[1]).ends_with("sendMessage"));
        assert_eq!(seen[1].body["message_thread_id"], 11);
        assert_eq!(bridge.session_topic("proj", "s1"), Some(11));
    }

    #[test]
    fn two_prompts_of_one_session_queue_one_topic() {
        let (bridge, _sink) = bridge_with("http://127.0.0.1:9");
        add_project(&bridge, "proj", true);
        bridge.forward("main", payload("s1", 1)).unwrap();
        bridge.forward("main", payload("s1", 2)).unwrap();
        bridge.forward("main", payload("s2", 1)).unwrap();
        let jobs = queued_edits(&bridge);
        let topics = jobs
            .iter()
            .filter(|job| matches!(job, Job::CreateTopic { .. }))
            .count();
        assert_eq!(topics, 2, "one per session: {jobs:?}");
        assert!(matches!(jobs[0], Job::CreateTopic { .. }));
        assert_eq!(jobs.len(), 5);
    }

    #[test]
    fn topic_creation_failure_falls_back_to_general() {
        let (base, server) = serve_script(vec![
            fail(
                400,
                "Bad Request: not enough rights to create a topic",
                json!({}),
            ),
            ok(json!({ "message_id": 8 })),
        ]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        bridge.forward("main", payload("s1", 1)).unwrap();
        drain(&bridge);
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].body.get("message_thread_id").is_none());
        assert_eq!(bridge.session_topic("proj", "s1"), None);
        assert_eq!(
            bridge.status("proj").problem.as_deref(),
            Some(PROBLEM_NO_TOPICS)
        );
        assert_eq!(
            bridge.lock().registry.prompt("s1#1").unwrap().messages[0],
            Some(8)
        );
    }

    #[test]
    fn too_long_message_is_dropped_with_a_problem() {
        let (base, server) = serve_script(vec![fail(
            400,
            "Bad Request: message is too long",
            json!({}),
        )]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        bind_topic(&bridge, "proj", "s1", 42);
        bridge.forward("main", payload("s1", 1)).unwrap();
        drain(&bridge);
        assert_eq!(server.join().unwrap().len(), 1, "no retry");
        assert!(bridge.lock().registry.prompt("s1#1").is_none());
        assert_eq!(
            bridge.status("proj").problem.as_deref(),
            Some("A question was too long for Telegram and was not sent")
        );
    }

    #[test]
    fn edit_not_modified_counts_as_success() {
        let (base, server) = serve_script(vec![fail(
            400,
            "Bad Request: message is not modified",
            json!({}),
        )]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        bridge.push_job(
            CHAT,
            Job::EditStatus {
                chat: CHAT,
                message_id: 5,
                html: "x".into(),
                keyboard: None,
            },
        );
        drain(&bridge);
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(bridge.status("proj").problem.is_none());
    }

    #[test]
    fn network_error_backs_off_and_retries() {
        let (base, server) = serve_script(vec![
            (502, json!({ "ok": false })),
            ok(json!({ "message_id": 4 })),
        ]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        bind_topic(&bridge, "proj", "s1", 42);
        bridge.forward("main", payload("s1", 1)).unwrap();
        drain(&bridge);
        assert_eq!(server.join().unwrap().len(), 2);
        assert_eq!(
            bridge.lock().registry.prompt("s1#1").unwrap().messages[0],
            Some(4)
        );
    }

    // ----- forwarding guards -----

    #[test]
    fn forward_needs_enabled_connected_project() {
        let (bridge, _sink) = bridge_with("http://127.0.0.1:9");
        assert!(bridge.forward("main", payload("s1", 1)).is_err(), "unknown");
        add_project(&bridge, "proj", false);
        assert!(
            bridge.forward("main", payload("s1", 1)).is_err(),
            "disabled"
        );
        let set = |enabled: bool, chat: Option<i64>| {
            let mut state = bridge.lock();
            let p = state.cfg.projects.get_mut("proj").unwrap();
            p.enabled = enabled;
            p.chat_id = chat;
        };
        set(true, None);
        assert!(
            bridge.forward("main", payload("s1", 1)).is_err(),
            "no group"
        );
        set(true, Some(CHAT));
        let mut empty = payload("s1", 1);
        empty.questions.clear();
        assert!(bridge.forward("main", empty).is_err(), "no questions");
        bridge.forward("main", payload("s1", 1)).unwrap();
        // The same prompt twice is queued once.
        bridge.forward("main", payload("s1", 1)).unwrap();
        let sends = queued_edits(&bridge)
            .iter()
            .filter(|job| matches!(job, Job::SendQuestion { .. }))
            .count();
        assert_eq!(sends, 1);
    }

    // ----- deliveries -----

    /// Inserts a prompt with a sent message, then simulates a tap on its first option.
    fn tap_first_option(bridge: &Arc<Bridge>, api: &BotApi) -> String {
        bridge.forward("main", payload("s1", 1)).unwrap();
        let token = {
            let mut state = bridge.lock();
            state.registry.set_message("s1#1", 0, 500);
            state.registry.prompt("s1#1").unwrap().tokens[0].clone()
        };
        // Drop the queued SendQuestion so only effects of the tap remain.
        bridge.lock().queue = SendQueue::default();
        let update = json!({
            "update_id": 1,
            "callback_query": {
                "id": "cb1",
                "from": { "id": 9, "username": "alice" },
                "data": format!("o:{token}:0"),
                "message": { "message_id": 500, "chat": { "id": CHAT } }
            }
        });
        bridge.process_update(api, TOKEN, &update);
        "s1#1".to_string()
    }

    fn queued_edits(bridge: &Arc<Bridge>) -> Vec<Job> {
        let mut state = bridge.lock();
        let mut jobs = Vec::new();
        let mut at = 10_000_000;
        while let Some((_, job)) = state.queue.next_ready(at) {
            jobs.push(job);
            at += 2_000;
        }
        jobs
    }

    #[test]
    fn deliver_to_missing_window_marks_failed() {
        let (base, server) = serve_script(vec![ok(json!(true))]);
        let (bridge, sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        sink.open.lock().unwrap().clear();
        let key = tap_first_option(&bridge, &bridge.api(TOKEN));
        server.join().unwrap();
        assert!(sink.events.lock().unwrap().is_empty());
        assert!(!bridge.lock().deliveries.contains_key(&key));
        let jobs = queued_edits(&bridge);
        assert!(
            matches!(&jobs[..], [Job::EditStatus { html, keyboard: None, .. }] if html.contains("expired")),
            "{jobs:?}"
        );
    }

    #[test]
    fn undelivered_prompt_times_out() {
        let (base, server) = serve_script(vec![ok(json!(true))]);
        let (bridge, sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        let key = tap_first_option(&bridge, &bridge.api(TOKEN));
        server.join().unwrap();
        {
            let events = sink.events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].0, "main");
            assert_eq!(events[0].1["key"], "s1#1");
            assert_eq!(events[0].1["sessionId"], "s1");
            assert_eq!(events[0].1["requestId"], 1);
            assert_eq!(events[0].1["by"], "@alice");
            assert_eq!(events[0].1["reply"]["kind"], "answered");
        }
        assert!(bridge.lock().deliveries.contains_key(&key));
        bridge.expire_deliveries(bridge.now_ms() + 1_000);
        assert!(bridge.lock().deliveries.contains_key(&key), "not yet due");
        bridge.expire_deliveries(bridge.now_ms() + DELIVERY_TIMEOUT_MS + 1_000);
        assert!(!bridge.lock().deliveries.contains_key(&key));
        let jobs = queued_edits(&bridge);
        assert!(
            matches!(&jobs[..], [Job::EditStatus { html, .. }] if html.contains("expired")),
            "{jobs:?}"
        );
    }

    #[test]
    fn answer_result_ok_edits_with_the_answer() {
        let (base, server) = serve_script(vec![ok(json!(true))]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        let key = tap_first_option(&bridge, &bridge.api(TOKEN));
        server.join().unwrap();
        bridge.answer_result(&key, true);
        assert!(!bridge.lock().deliveries.contains_key(&key));
        let jobs = queued_edits(&bridge);
        assert!(
            matches!(&jobs[..], [Job::EditStatus { html, .. }] if html.contains("Answered by @alice")),
            "{jobs:?}"
        );
    }

    // ----- connect and link -----

    fn connect_update(code: &str, chat: i64) -> Value {
        json!({
            "update_id": 2,
            "message": {
                "message_id": 1,
                "from": { "id": 9 },
                "chat": { "id": chat, "type": "supergroup", "title": "Team" },
                "text": format!("/connect@my_bot {code}")
            }
        })
    }

    #[test]
    fn connect_records_chat_and_rights() {
        let (base, server) = serve_script(vec![
            ok(json!({ "id": CHAT - 1, "is_forum": true })),
            ok(json!({ "status": "administrator", "can_manage_topics": true })),
        ]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", false);
        bridge.lock().cfg.projects.get_mut("proj").unwrap().chat_id = None;
        let link = bridge.connect_link_for_test("proj");
        bridge.process_update(&bridge.api(TOKEN), TOKEN, &connect_update(&link, CHAT - 1));
        server.join().unwrap();
        let status = bridge.status("proj");
        assert_eq!(status.chat_id, Some(CHAT - 1));
        assert_eq!(status.chat_title.as_deref(), Some("Team"));
        assert_eq!(status.is_forum, Some(true));
        assert_eq!(status.can_manage_topics, Some(true));
        assert!(status.problem.is_none());
        let jobs = queued_edits(&bridge);
        assert!(
            matches!(&jobs[..], [Job::Notice { thread: None, html }] if html.contains("Connected")),
            "{jobs:?}"
        );
    }

    #[test]
    fn connect_with_another_bots_update_is_ignored() {
        let (bridge, _sink) = bridge_with("http://127.0.0.1:9");
        add_project(&bridge, "proj", false);
        bridge.lock().cfg.projects.get_mut("proj").unwrap().chat_id = None;
        let link = bridge.connect_link_for_test("proj");
        // The update arrived through a different bot's poller.
        bridge.process_update(
            &bridge.api("999:other"),
            "999:other",
            &connect_update(&link, CHAT - 1),
        );
        assert_eq!(bridge.status("proj").chat_id, None);
    }

    #[test]
    fn link_requires_the_chat_of_the_issuing_project() {
        let (bridge, _sink) = bridge_with("http://127.0.0.1:9");
        add_project(&bridge, "a", true);
        add_project(&bridge, "b", true);
        bridge.lock().cfg.projects.get_mut("b").unwrap().chat_id = Some(CHAT - 5);
        let link = |bridge: &Arc<Bridge>, chat: i64, thread: i64| {
            let code = bridge.link_code("a", "s1").map(|c| c.code).unwrap();
            json!({
                "update_id": 3,
                "message": {
                    "message_id": 2,
                    "from": { "id": 9 },
                    "chat": { "id": chat, "type": "supergroup" },
                    "is_topic_message": true,
                    "message_thread_id": thread,
                    "text": format!("/link {code}")
                }
            })
        };
        let api = bridge.api(TOKEN);
        // Used in project b's group: refused.
        bridge.process_update(&api, TOKEN, &link(&bridge, CHAT - 5, 77));
        assert_eq!(bridge.session_topic("a", "s1"), None);
        assert_eq!(bridge.session_topic("b", "s1"), None);
        // Used in its own group: bound.
        bridge.process_update(&api, TOKEN, &link(&bridge, CHAT, 78));
        assert_eq!(bridge.session_topic("a", "s1"), Some(78));
    }

    // ----- poller thread -----

    #[test]
    fn poller_skips_backlog_then_stops_on_unauthorized() {
        let (base, server) = serve_script(vec![
            ok(json!([{ "update_id": 5 }])),
            (
                401,
                json!({ "ok": false, "error_code": 401, "description": "Unauthorized" }),
            ),
        ]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        bridge.lock().cfg.projects.get_mut("proj").unwrap().chat_id = None;
        let stop = Arc::new(AtomicBool::new(false));
        Arc::clone(&bridge).run_poller(TOKEN.to_string(), stop);
        let seen = server.join().unwrap();
        assert_eq!(seen[0].body["offset"], -1);
        assert_eq!(seen[0].body["timeout"], 0);
        assert_eq!(seen[1].body["offset"], 6);
        assert_eq!(seen[1].body["timeout"], 50);
        assert_eq!(
            bridge.status("proj").problem.as_deref(),
            Some("Bot token was rejected")
        );
    }

    #[test]
    fn answer_callback_is_sent_before_anything_is_queued() {
        let (base, server) = serve_script(vec![ok(json!(true))]);
        let (bridge, _sink) = bridge_with(&base);
        add_project(&bridge, "proj", true);
        // A tap with an unknown token still gets its callback answered, not queued.
        let update = json!({
            "update_id": 1,
            "callback_query": {
                "id": "cb9",
                "from": { "id": 9 },
                "data": "o:nosuchtoken:0",
                "message": { "message_id": 1, "chat": { "id": CHAT } }
            }
        });
        bridge.process_update(&bridge.api(TOKEN), TOKEN, &update);
        let seen = server.join().unwrap();
        assert!(method_of(&seen[0]).ends_with("answerCallbackQuery"));
        assert_eq!(seen[0].body["callback_query_id"], "cb9");
        assert!(bridge.lock().queue.next_wake(0).is_none());
    }

    impl Bridge {
        /// Issues a connect code the way `connect_link` does, without starting pollers.
        fn connect_link_for_test(&self, project: &str) -> String {
            let now = self.now_ms();
            let mut state = self.lock();
            state.codes.issue(
                CodePurpose::Connect {
                    project: project.to_string(),
                },
                now,
            )
        }
    }
}
