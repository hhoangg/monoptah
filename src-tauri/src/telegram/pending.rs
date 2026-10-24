//! Registry of questions that were forwarded to Telegram and are still waiting.
//!
//! Pure state machine: no I/O, no threads, time is passed in. Updates from Telegram go in
//! through `handle_update` and come out as `Effect`s the caller performs. Text from
//! Telegram is data only: it can become a custom answer string and nothing else.
//!
//! Several bots can sit in one group and each sees every message, so each update comes
//! with the `Receiver` that got it. A command addressed to another bot (`/link@other`) is
//! ignored, a code is only spent by a bot that serves the code's project, and a text reply
//! only answers a question that the receiving bot sent.

use super::config::{CodePurpose, Codes};
use super::render::{
    is_other, keyboard, new_token, question_html, status_html, ForwardPayload, QuestionIn,
    CUSTOM_OPTION_ID,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub struct PendingPrompt {
    pub owner_window: String,
    pub payload: ForwardPayload,
    pub chat: i64,
    /// Username of the bot that sent the question messages. Only that bot's poller may
    /// treat a reply to them as an answer.
    pub bot: String,
    /// Telegram message id per question, set once the message was sent.
    pub messages: Vec<Option<i64>>,
    /// Callback token per question.
    pub tokens: Vec<String>,
    pub selected: Vec<BTreeSet<String>>,
    pub custom: Vec<Option<String>>,
    pub done: Vec<bool>,
    pub awaiting_text: Vec<bool>,
}

impl PendingPrompt {
    pub fn new(owner_window: String, payload: ForwardPayload, chat: i64, bot: String) -> Self {
        let count = payload.questions.len();
        Self {
            owner_window,
            payload,
            chat,
            bot,
            messages: vec![None; count],
            tokens: (0..count).map(|_| new_token()).collect(),
            selected: vec![BTreeSet::new(); count],
            custom: vec![None; count],
            done: vec![false; count],
            awaiting_text: vec![false; count],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    AnswerCallback {
        id: String,
        text: String,
    },
    Edit {
        /// The project whose bot sent the message, so the edit goes out with its token.
        project: String,
        chat: i64,
        message_id: i64,
        html: String,
        keyboard: Option<Value>,
    },
    Deliver {
        window: String,
        session_id: String,
        request_id: u64,
        /// `UserQuestionReply` JSON.
        reply: Value,
        by: String,
    },
    Connected {
        project: String,
        chat: i64,
        title: String,
    },
    Linked {
        project: String,
        session_id: String,
        thread: i64,
    },
    Reply {
        /// The project whose bot answers; `None` for replies to commands, which the bot
        /// that received the command sends.
        project: Option<String>,
        chat: i64,
        thread: Option<i64>,
        html: String,
    },
    /// A configured group became a supergroup (Telegram's service message).
    Migrated {
        from: i64,
        to: i64,
    },
}

/// A completed prompt whose reply was handed to the frontend and awaits its result.
struct Delivering {
    /// Who answered; the source of truth for the final status text.
    by: String,
    /// The question that completed the prompt; its message is edited after delivery.
    last: usize,
    /// Set when the desktop closed the prompt while the reply was in flight.
    closed_as: Option<String>,
}

/// The bot whose poller received an update, and the projects that use that bot.
pub struct Receiver<'a> {
    /// Bot username without the `@`; empty when unknown.
    pub username: &'a str,
    pub projects: &'a BTreeSet<String>,
}

#[derive(Default)]
pub struct Registry {
    /// "<sessionId>#<requestId>" -> prompt
    prompts: BTreeMap<String, PendingPrompt>,
    /// token -> (key, question index); only for questions that are not done
    tokens: BTreeMap<String, (String, usize)>,
    delivering: BTreeMap<String, Delivering>,
}

pub const EXPIRED: &str = "This question has expired";
const MAX_CUSTOM_CHARS: usize = 2000;

pub fn key_of(session_id: &str, request_id: u64) -> String {
    format!("{session_id}#{request_id}")
}

fn answer(id: &str, text: &str) -> Effect {
    Effect::AnswerCallback {
        id: id.to_string(),
        text: text.to_string(),
    }
}

/// What a tap result looks like before the mandatory `AnswerCallback` is put in front.
type Outcome = (String, Vec<Effect>);

fn outcome(toast: &str, effects: Vec<Effect>) -> Outcome {
    (toast.to_string(), effects)
}

/// "@username", else the display name, else "someone".
fn actor(from: &Value) -> String {
    if let Some(username) = from["username"].as_str().filter(|u| !u.is_empty()) {
        return format!("@{username}");
    }
    let first = from["first_name"].as_str().unwrap_or("").trim();
    let last = from["last_name"].as_str().unwrap_or("").trim();
    let name = format!("{first} {last}").trim().to_string();
    if name.is_empty() {
        "someone".to_string()
    } else {
        name
    }
}

/// `/name@bot arg` -> ("name", Some("bot"), "arg"); a bare command has no bot.
fn parse_command(text: &str) -> Option<(String, Option<&str>, &str)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (head, arg) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(head, arg)| (head, arg.trim()));
    let (name, bot) = match head.split_once('@') {
        Some((name, bot)) => (name, Some(bot)),
        None => (head, None),
    };
    Some((name.to_ascii_lowercase(), bot, arg))
}

fn project_of(purpose: &CodePurpose) -> &str {
    match purpose {
        CodePurpose::Connect { project } | CodePurpose::Link { project, .. } => project,
    }
}

/// A live code issued for a project of another bot. Such a code is neither spent nor
/// answered, so the bot it is for can still use it.
fn is_foreign_code(codes: &Codes, code: &str, now_ms: u64, receiver: &Receiver<'_>) -> bool {
    codes
        .peek(code, now_ms)
        .is_some_and(|purpose| !receiver.projects.contains(project_of(purpose)))
}

fn is_group(chat: &Value) -> bool {
    matches!(chat["type"].as_str(), Some("group" | "supergroup"))
}

fn thread_of(msg: &Value) -> Option<i64> {
    if msg["is_topic_message"].as_bool() == Some(true) {
        msg["message_thread_id"].as_i64()
    } else {
        None
    }
}

/// The id a custom answer is sent under: the question's own "Other" option when it has one
/// (the desktop form does the same, and providers such as Cursor drop `__custom__` as an
/// unknown option), else the app-wide custom marker.
fn custom_id(q: &QuestionIn) -> &str {
    q.options
        .iter()
        .find(|option| is_other(option))
        .map_or(CUSTOM_OPTION_ID, |option| option.id.as_str())
}

fn is_custom_id(q: &QuestionIn, id: &str) -> bool {
    id == CUSTOM_OPTION_ID || id == custom_id(q)
}

/// Option ids in option order, then the custom marker.
fn selected_ids(prompt: &PendingPrompt, q_index: usize) -> Vec<String> {
    let selected = &prompt.selected[q_index];
    let mut ids: Vec<String> = prompt.payload.questions[q_index]
        .options
        .iter()
        .filter(|option| selected.contains(&option.id))
        .map(|option| option.id.clone())
        .collect();
    if selected.contains(CUSTOM_OPTION_ID) && !ids.iter().any(|id| id == CUSTOM_OPTION_ID) {
        ids.push(CUSTOM_OPTION_ID.to_string());
    }
    ids
}

fn selected_labels(prompt: &PendingPrompt, q_index: usize) -> Vec<String> {
    let q = &prompt.payload.questions[q_index];
    selected_ids(prompt, q_index)
        .iter()
        .filter_map(|id| {
            if is_custom_id(q, id) {
                if let Some(text) = &prompt.custom[q_index] {
                    return Some(text.clone());
                }
            }
            q.options
                .iter()
                .find(|option| &option.id == id)
                .map(|option| option.label.clone())
        })
        .collect()
}

/// Same shape as `buildQuestionReply` in `userQuestion.ts`: only answered questions are
/// included, `custom` is omitted when empty, and no answered question means skipped.
fn build_reply(prompt: &PendingPrompt) -> Value {
    let mut answers = serde_json::Map::new();
    let mut custom = serde_json::Map::new();
    for (index, q) in prompt.payload.questions.iter().enumerate() {
        let ids = selected_ids(prompt, index);
        if ids.is_empty() {
            continue;
        }
        if ids.iter().any(|id| is_custom_id(q, id)) {
            if let Some(text) = &prompt.custom[index] {
                custom.insert(q.id.clone(), json!(text));
            }
        }
        answers.insert(q.id.clone(), json!(ids));
    }
    if answers.is_empty() {
        return json!({ "kind": "skipped" });
    }
    let mut reply = json!({ "kind": "answered", "answers": answers });
    if !custom.is_empty() {
        reply["custom"] = Value::Object(custom);
    }
    reply
}

fn outcome_text(prompt: &PendingPrompt, q_index: usize, by: &str) -> String {
    let labels = selected_labels(prompt, q_index);
    if labels.is_empty() {
        format!("Skipped by {by}")
    } else {
        format!("Answered by {by}: {}", labels.join(", "))
    }
}

/// Replaces a question's message with a status line and no buttons.
fn edit_status(prompt: &PendingPrompt, q_index: usize, status: &str) -> Option<Effect> {
    let message_id = prompt.messages.get(q_index).copied().flatten()?;
    Some(Effect::Edit {
        project: prompt.payload.project.clone(),
        chat: prompt.chat,
        message_id,
        html: status_html(&prompt.payload, q_index, status),
        keyboard: None,
    })
}

fn edit_all(prompt: &PendingPrompt, status: &str) -> Vec<Effect> {
    (0..prompt.payload.questions.len())
        .filter_map(|index| edit_status(prompt, index, status))
        .collect()
}

/// Re-renders an open question with its current selection.
fn edit_open(prompt: &PendingPrompt, q_index: usize) -> Option<Effect> {
    let message_id = prompt.messages.get(q_index).copied().flatten()?;
    let q = &prompt.payload.questions[q_index];
    let selected = &prompt.selected[q_index];
    Some(Effect::Edit {
        project: prompt.payload.project.clone(),
        chat: prompt.chat,
        message_id,
        html: question_html(&prompt.payload, q_index, selected),
        keyboard: Some(keyboard(&prompt.tokens[q_index], q, selected)),
    })
}

impl Registry {
    pub fn insert(&mut self, mut prompt: PendingPrompt) -> String {
        let key = key_of(&prompt.payload.session_id, prompt.payload.request_id);
        self.forget(&key);
        let count = prompt.payload.questions.len();
        prompt.messages.resize(count, None);
        prompt.tokens.resize_with(count, new_token);
        prompt.selected.resize(count, BTreeSet::new());
        prompt.custom.resize(count, None);
        prompt.done.resize(count, false);
        prompt.awaiting_text.resize(count, false);
        for (index, token) in prompt.tokens.iter().enumerate() {
            if !prompt.done[index] {
                self.tokens.insert(token.clone(), (key.clone(), index));
            }
        }
        self.prompts.insert(key.clone(), prompt);
        key
    }

    pub fn prompt(&self, key: &str) -> Option<&PendingPrompt> {
        self.prompts.get(key)
    }

    /// Whether any pending prompt belongs to this session, i.e. a topic for it is still wanted.
    pub fn has_prompt_for_session(&self, project: &str, session_id: &str) -> bool {
        self.prompts
            .values()
            .any(|p| p.payload.project == project && p.payload.session_id == session_id)
    }

    /// Whether the prompt stored under `key` is the one `payload` describes, asked by
    /// `owner`. Request ids restart when a window reloads, so a key alone does not say
    /// that: the owner window, the question ids and their option ids must all match too.
    pub fn is_same_prompt(&self, key: &str, owner: &str, payload: &ForwardPayload) -> bool {
        let Some(old) = self.prompts.get(key) else {
            return false;
        };
        let shape = |payload: &ForwardPayload| -> Vec<(String, Vec<String>)> {
            payload
                .questions
                .iter()
                .map(|q| {
                    let options = q.options.iter().map(|option| option.id.clone());
                    (q.id.clone(), options.collect())
                })
                .collect()
        };
        old.owner_window == owner && shape(&old.payload) == shape(payload)
    }

    /// Drops a prompt that a new one is about to replace under the same key. Its messages
    /// are edited to the expired status and its tokens stop working, even when its reply
    /// was already with a window.
    pub fn expire(&mut self, key: &str) -> Vec<Effect> {
        match self.forget(key) {
            Some(prompt) => edit_all(&prompt, EXPIRED),
            None => Vec::new(),
        }
    }

    /// Forgets every prompt of `project` without touching Telegram: used when the project
    /// loses its bot or its group, so no prompt is left that routes replies or edits through
    /// a bot that no longer belongs to it. Returns the keys that were dropped.
    pub fn discard_project(&mut self, project: &str) -> Vec<String> {
        let keys: Vec<String> = self
            .prompts
            .iter()
            .filter(|(_, prompt)| prompt.payload.project == project)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &keys {
            self.forget(key);
        }
        keys
    }

    /// A group became a supergroup: its prompts now live under the new chat id, so taps and
    /// replies from the new chat still match and status edits go to the right place.
    pub fn migrate_chat(&mut self, from: i64, to: i64) {
        for prompt in self.prompts.values_mut().filter(|p| p.chat == from) {
            prompt.chat = to;
        }
    }

    /// Records the message sent for a question, but only while the slot still has the
    /// callback token the message was built with. A prompt that was replaced meanwhile has
    /// new tokens, so the old message must not become the new prompt's. Returns whether the
    /// id was stored.
    pub fn set_message(
        &mut self,
        key: &str,
        q_index: usize,
        message_id: i64,
        expected_token: &str,
    ) -> bool {
        let Some(prompt) = self.prompts.get_mut(key) else {
            return false;
        };
        if prompt.tokens.get(q_index).map(String::as_str) != Some(expected_token) {
            return false;
        }
        match prompt.messages.get_mut(q_index) {
            Some(slot) => {
                *slot = Some(message_id);
                true
            }
            None => false,
        }
    }

    /// Test shortcut: stores a message id under the slot's own current token.
    #[cfg(test)]
    pub fn set_message_for_test(&mut self, key: &str, q_index: usize, message_id: i64) {
        let token = self.prompts[key].tokens[q_index].clone();
        assert!(self.set_message(key, q_index, message_id, &token));
    }

    /// The desktop answered, skipped or cancelled the prompt (or it expired).
    ///
    /// A prompt whose reply is already with the frontend is not closed here: clearing the
    /// session's pending question is exactly what a Telegram answer does, so the result
    /// from `delivered` decides. The status is kept in case that result is a failure.
    pub fn close(&mut self, session_id: &str, request_id: u64, status: &str) -> Vec<Effect> {
        let key = key_of(session_id, request_id);
        if let Some(delivering) = self.delivering.get_mut(&key) {
            delivering
                .closed_as
                .get_or_insert_with(|| status.to_string());
            return Vec::new();
        }
        match self.forget(&key) {
            Some(prompt) => edit_all(&prompt, status),
            None => Vec::new(),
        }
    }

    pub fn handle_update(
        &mut self,
        update: &Value,
        configured_chats: &BTreeSet<i64>,
        receiver: &Receiver<'_>,
        codes: &mut Codes,
        now_ms: u64,
    ) -> Vec<Effect> {
        if let Some(callback) = update.get("callback_query") {
            return self.handle_callback(callback, configured_chats);
        }
        if let Some(message) = update.get("message") {
            return self.handle_message(message, configured_chats, receiver, codes, now_ms);
        }
        Vec::new()
    }

    /// The frontend's result for a `Deliver`. Who answered comes from the stored state, so
    /// the poller does not need to know.
    pub fn delivered(&mut self, key: &str, ok: bool) -> Vec<Effect> {
        let Some(delivering) = self.delivering.remove(key) else {
            return Vec::new();
        };
        let Some(prompt) = self.forget(key) else {
            return Vec::new();
        };
        if ok {
            let status = outcome_text(&prompt, delivering.last, &delivering.by);
            edit_status(&prompt, delivering.last, &status)
                .into_iter()
                .collect()
        } else {
            let status = delivering.closed_as.as_deref().unwrap_or(EXPIRED);
            edit_all(&prompt, status)
        }
    }

    fn forget(&mut self, key: &str) -> Option<PendingPrompt> {
        self.tokens.retain(|_, (owner, _)| owner != key);
        self.delivering.remove(key);
        self.prompts.remove(key)
    }

    fn handle_callback(&mut self, cq: &Value, chats: &BTreeSet<i64>) -> Vec<Effect> {
        let Some(id) = cq["id"].as_str() else {
            return Vec::new();
        };
        let Some(chat) = cq.pointer("/message/chat/id").and_then(Value::as_i64) else {
            return Vec::new();
        };
        if !chats.contains(&chat) {
            return Vec::new();
        }
        let data = cq["data"].as_str().unwrap_or("");
        let mut parts = data.splitn(3, ':');
        let (kind, token, arg) = (
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
            parts.next(),
        );
        let target = self
            .tokens
            .get(token)
            .filter(|(key, _)| self.prompts.get(key).is_some_and(|p| p.chat == chat))
            .cloned();
        let Some((key, q_index)) = target else {
            return vec![answer(id, EXPIRED)];
        };
        let by = actor(&cq["from"]);
        let thread = cq
            .pointer("/message/message_thread_id")
            .and_then(Value::as_i64);
        let (toast, mut rest) = match kind {
            "o" => self.tap_option(&key, q_index, arg, &by, thread),
            "s" => self.tap_send(&key, q_index, &by),
            "k" => self.tap_skip(&key, q_index, &by),
            "c" => self.ask_text(&key, q_index, thread),
            _ => outcome("Unknown action", Vec::new()),
        };
        let mut effects = vec![answer(id, &toast)];
        effects.append(&mut rest);
        effects
    }

    fn tap_option(
        &mut self,
        key: &str,
        q_index: usize,
        arg: Option<&str>,
        by: &str,
        thread: Option<i64>,
    ) -> Outcome {
        let Some(prompt) = self.prompts.get_mut(key) else {
            return outcome(EXPIRED, Vec::new());
        };
        let q = &prompt.payload.questions[q_index];
        let Some(option) = arg
            .and_then(|arg| arg.parse::<usize>().ok())
            .and_then(|index| q.options.get(index))
        else {
            return outcome("This option is not available", Vec::new());
        };
        if q.allow_custom && is_other(option) {
            return self.ask_text(key, q_index, thread);
        }
        let id = option.id.clone();
        if q.multi_select {
            let selected = &mut prompt.selected[q_index];
            if !selected.remove(&id) {
                selected.insert(id);
            }
            return outcome("", edit_open(prompt, q_index).into_iter().collect());
        }
        prompt.selected[q_index] = BTreeSet::from([id]);
        prompt.custom[q_index] = None;
        outcome("Recorded", self.finish_question(key, q_index, by))
    }

    fn tap_send(&mut self, key: &str, q_index: usize, by: &str) -> Outcome {
        let Some(prompt) = self.prompts.get(key) else {
            return outcome(EXPIRED, Vec::new());
        };
        if !prompt.payload.questions[q_index].multi_select {
            return outcome("Unknown action", Vec::new());
        }
        if prompt.selected[q_index].is_empty() {
            return outcome("Select at least one option first", Vec::new());
        }
        outcome("Recorded", self.finish_question(key, q_index, by))
    }

    fn tap_skip(&mut self, key: &str, q_index: usize, by: &str) -> Outcome {
        let Some(prompt) = self.prompts.get_mut(key) else {
            return outcome(EXPIRED, Vec::new());
        };
        prompt.selected[q_index].clear();
        prompt.custom[q_index] = None;
        outcome("Skipped", self.finish_question(key, q_index, by))
    }

    /// The "Other" button: the next reply to the question message is the answer.
    fn ask_text(&mut self, key: &str, q_index: usize, thread: Option<i64>) -> Outcome {
        let Some(prompt) = self.prompts.get_mut(key) else {
            return outcome(EXPIRED, Vec::new());
        };
        if !prompt.payload.questions[q_index].allow_custom {
            return outcome("This question does not take a custom answer", Vec::new());
        }
        prompt.awaiting_text[q_index] = true;
        let hint = Effect::Reply {
            project: Some(prompt.payload.project.clone()),
            chat: prompt.chat,
            thread,
            html: "Reply to the question message above with your answer.".to_string(),
        };
        outcome("Reply to the question message with your answer", vec![hint])
    }

    /// Marks a question done. The last one hands the whole prompt to the frontend; its
    /// message is edited once the result is known, the others at once.
    fn finish_question(&mut self, key: &str, q_index: usize, by: &str) -> Vec<Effect> {
        let Some(prompt) = self.prompts.get_mut(key) else {
            return Vec::new();
        };
        prompt.done[q_index] = true;
        prompt.awaiting_text[q_index] = false;
        self.tokens.remove(&prompt.tokens[q_index]);
        if !prompt.done.iter().all(|done| *done) {
            let status = outcome_text(prompt, q_index, by);
            return edit_status(prompt, q_index, &status).into_iter().collect();
        }
        let deliver = Effect::Deliver {
            window: prompt.owner_window.clone(),
            session_id: prompt.payload.session_id.clone(),
            request_id: prompt.payload.request_id,
            reply: build_reply(prompt),
            by: by.to_string(),
        };
        self.delivering.insert(
            key.to_string(),
            Delivering {
                by: by.to_string(),
                last: q_index,
                closed_as: None,
            },
        );
        vec![deliver]
    }

    fn handle_message(
        &mut self,
        msg: &Value,
        chats: &BTreeSet<i64>,
        receiver: &Receiver<'_>,
        codes: &mut Codes,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(chat) = msg.pointer("/chat/id").and_then(Value::as_i64) else {
            return Vec::new();
        };
        if let Some(effect) = migration(msg, chat, chats) {
            return vec![effect];
        }
        if msg.pointer("/from/is_bot").and_then(Value::as_bool) == Some(true) {
            return Vec::new();
        }
        let text = msg["text"].as_str().unwrap_or("");
        let configured = chats.contains(&chat);
        match parse_command(text) {
            Some((name, bot, arg)) if matches!(name.as_str(), "start" | "connect" | "link") => {
                if bot.is_some_and(|bot| !bot.eq_ignore_ascii_case(receiver.username)) {
                    return Vec::new();
                }
                let ctx = Command {
                    chat,
                    configured,
                    receiver,
                    now_ms,
                };
                return if name == "link" {
                    link(msg, &ctx, arg, codes)
                } else {
                    connect(msg, &ctx, arg, codes)
                };
            }
            _ => {}
        }
        if !configured {
            return Vec::new();
        }
        self.handle_text(msg, chat, text, receiver)
    }

    /// A reply to a question message the user asked to answer with text. Only a reply to
    /// exactly that message counts: topic messages all carry a `reply_to_message` that
    /// points at the topic creation message, and those must not become answers.
    fn handle_text(
        &mut self,
        msg: &Value,
        chat: i64,
        text: &str,
        receiver: &Receiver<'_>,
    ) -> Vec<Effect> {
        let Some(reply_to) = msg
            .pointer("/reply_to_message/message_id")
            .and_then(Value::as_i64)
        else {
            return Vec::new();
        };
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let Some((key, q_index)) = self.prompts.iter().find_map(|(key, prompt)| {
            // Another bot's question can be replied to in the same group; not ours to answer.
            if prompt.chat != chat || !prompt.bot.eq_ignore_ascii_case(receiver.username) {
                return None;
            }
            let index = prompt.messages.iter().position(|m| *m == Some(reply_to))?;
            let open = !prompt.done[index]
                && prompt.awaiting_text[index]
                && prompt.payload.questions[index].allow_custom;
            open.then(|| (key.clone(), index))
        }) else {
            return Vec::new();
        };
        let by = actor(&msg["from"]);
        let Some(prompt) = self.prompts.get_mut(&key) else {
            return Vec::new();
        };
        prompt.custom[q_index] = Some(text.chars().take(MAX_CUSTOM_CHARS).collect());
        prompt.awaiting_text[q_index] = false;
        let custom = custom_id(&prompt.payload.questions[q_index]).to_string();
        if prompt.payload.questions[q_index].multi_select {
            prompt.selected[q_index].insert(custom);
            return edit_open(prompt, q_index).into_iter().collect();
        }
        prompt.selected[q_index] = BTreeSet::from([custom]);
        self.finish_question(&key, q_index, &by)
    }
}

/// The service message Telegram posts when a group is upgraded: `migrate_to_chat_id` in
/// the old chat, `migrate_from_chat_id` in the new one. Only a chat that is configured
/// (the old one, in either form) can move, so an unrelated group cannot rewrite the config.
fn migration(msg: &Value, chat: i64, chats: &BTreeSet<i64>) -> Option<Effect> {
    let (from, to) = match (
        msg["migrate_to_chat_id"].as_i64(),
        msg["migrate_from_chat_id"].as_i64(),
    ) {
        (Some(to), _) => (chat, to),
        (None, Some(from)) => (from, chat),
        (None, None) => return None,
    };
    (from != to && chats.contains(&from)).then_some(Effect::Migrated { from, to })
}

fn reply(chat: i64, thread: Option<i64>, html: &str) -> Effect {
    Effect::Reply {
        project: None,
        chat,
        thread,
        html: html.to_string(),
    }
}

/// What `connect` and `link` need to know about the command they handle.
struct Command<'a> {
    chat: i64,
    configured: bool,
    receiver: &'a Receiver<'a>,
    now_ms: u64,
}

/// `/start <code>` or `/connect <code>` in a group: the one place an unconfigured chat is
/// listened to. A bad code is answered only in an already configured chat.
fn connect(msg: &Value, ctx: &Command<'_>, code: &str, codes: &mut Codes) -> Vec<Effect> {
    if code.is_empty() || !is_group(&msg["chat"]) {
        return Vec::new();
    }
    if is_foreign_code(codes, code, ctx.now_ms, ctx.receiver) {
        return Vec::new();
    }
    // Only a Connect code is spent here; a Link code typed into /connect stays valid.
    if matches!(
        codes.peek(code, ctx.now_ms),
        Some(CodePurpose::Connect { .. })
    ) {
        if let Some(CodePurpose::Connect { project }) = codes.take(code, ctx.now_ms) {
            return vec![Effect::Connected {
                project,
                chat: ctx.chat,
                title: msg
                    .pointer("/chat/title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            }];
        }
    }
    if ctx.configured {
        return vec![reply(
            ctx.chat,
            thread_of(msg),
            "That code is not valid or has expired.",
        )];
    }
    Vec::new()
}

/// `/link <code>` inside a topic of a configured group.
fn link(msg: &Value, ctx: &Command<'_>, code: &str, codes: &mut Codes) -> Vec<Effect> {
    if !ctx.configured || is_foreign_code(codes, code, ctx.now_ms, ctx.receiver) {
        return Vec::new();
    }
    let Some(thread) = thread_of(msg) else {
        return vec![reply(
            ctx.chat,
            None,
            "Send /link inside the topic you want to link, not in General.",
        )];
    };
    if matches!(codes.peek(code, ctx.now_ms), Some(CodePurpose::Link { .. })) {
        if let Some(CodePurpose::Link {
            project,
            session_id,
        }) = codes.take(code, ctx.now_ms)
        {
            return vec![Effect::Linked {
                project,
                session_id,
                thread,
            }];
        }
    }
    vec![reply(
        ctx.chat,
        Some(thread),
        "That code is not valid or has expired.",
    )]
}

#[cfg(test)]
mod tests {
    use super::super::render::{OptionIn, QuestionIn};
    use super::*;

    const CHAT: i64 = -1001;
    const WINDOW: &str = "main";
    const BOT: &str = "my_bot";

    fn option(id: &str, label: &str) -> OptionIn {
        OptionIn {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    fn question(id: &str, multi: bool, custom: bool) -> QuestionIn {
        QuestionIn {
            id: id.into(),
            header: None,
            prompt: format!("Prompt {id}"),
            multi_select: multi,
            allow_custom: custom,
            options: vec![
                option("a", "Alpha"),
                option("b", "Beta"),
                option("c", "Gamma"),
            ],
        }
    }

    fn payload(session: &str, request_id: u64, questions: Vec<QuestionIn>) -> ForwardPayload {
        ForwardPayload {
            project: "proj".into(),
            session_id: session.into(),
            request_id,
            topic_title: "Topic".into(),
            header: "proj - Session".into(),
            context: "ctx".into(),
            delay_secs: 300,
            questions,
        }
    }

    fn chats() -> BTreeSet<i64> {
        [CHAT].into()
    }

    /// Inserts a prompt whose question `i` has message id `100 * (k + 1) + i`, where `k`
    /// is the number of prompts inserted before. Returns the key.
    fn add(reg: &mut Registry, session: &str, request_id: u64, qs: Vec<QuestionIn>) -> String {
        let count = qs.len();
        let prompt = PendingPrompt::new(
            WINDOW.into(),
            payload(session, request_id, qs),
            CHAT,
            BOT.into(),
        );
        let key = reg.insert(prompt);
        let base = 100 * (reg.prompts.len() as i64);
        for i in 0..count {
            let token = reg.prompts[&key].tokens[i].clone();
            assert!(reg.set_message(&key, i, base + i as i64, &token));
        }
        key
    }

    #[test]
    fn set_message_needs_the_slots_current_token() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s", 1, vec![question("q1", false, false)]);
        let old = token(&reg, &key, 0);
        // The same key comes back as a new prompt with new tokens.
        reg.expire(&key);
        let fresh = PendingPrompt::new(
            WINDOW.into(),
            payload("s", 1, vec![question("q1", false, false)]),
            CHAT,
            BOT.into(),
        );
        reg.insert(fresh);
        assert!(!reg.set_message(&key, 0, 900, &old), "stale token");
        assert_eq!(reg.prompts[&key].messages[0], None);
        assert!(!reg.set_message("missing#1", 0, 900, &old));
        assert!(!reg.set_message(&key, 5, 900, &old), "no such question");
        let current = token(&reg, &key, 0);
        assert!(reg.set_message(&key, 0, 901, &current));
        assert_eq!(msg_id(&reg, &key, 0), 901);
    }

    fn token(reg: &Registry, key: &str, q: usize) -> String {
        reg.prompts[key].tokens[q].clone()
    }

    fn msg_id(reg: &Registry, key: &str, q: usize) -> i64 {
        reg.prompts[key].messages[q].unwrap()
    }

    fn press(
        reg: &mut Registry,
        key: &str,
        q: usize,
        kind: &str,
        arg: Option<usize>,
    ) -> Vec<Effect> {
        let token = token(reg, key, q);
        match arg {
            Some(arg) => tap(reg, &format!("{kind}:{token}:{arg}")),
            None => tap(reg, &format!("{kind}:{token}")),
        }
    }

    fn tap(reg: &mut Registry, data: &str) -> Vec<Effect> {
        tap_in(reg, CHAT, data)
    }

    fn tap_in(reg: &mut Registry, chat: i64, data: &str) -> Vec<Effect> {
        let update = json!({"update_id": 1, "callback_query": {
            "id": "cb1",
            "from": {"id": 5, "username": "alice", "first_name": "Alice"},
            "data": data,
            "message": {"message_id": 100, "chat": {"id": chat, "type": "supergroup"},
                "message_thread_id": 9, "is_topic_message": true}
        }});
        let receiver = Receiver {
            username: BOT,
            projects: &projects(),
        };
        reg.handle_update(&update, &chats(), &receiver, &mut Codes::default(), 0)
    }

    /// The projects the receiving bot serves in these tests.
    fn projects() -> BTreeSet<String> {
        ["proj", "p"].into_iter().map(String::from).collect()
    }

    fn say(reg: &mut Registry, msg: Value) -> Vec<Effect> {
        say_with(reg, msg, &mut Codes::default())
    }

    fn say_with(reg: &mut Registry, msg: Value, codes: &mut Codes) -> Vec<Effect> {
        say_as(reg, msg, codes, BOT)
    }

    fn say_as(reg: &mut Registry, msg: Value, codes: &mut Codes, bot: &str) -> Vec<Effect> {
        let update = json!({"update_id": 2, "message": msg});
        let receiver = Receiver {
            username: bot,
            projects: &projects(),
        };
        reg.handle_update(&update, &chats(), &receiver, codes, 0)
    }

    fn text_msg(chat: i64, text: &str, reply_to: Option<i64>) -> Value {
        let mut msg = json!({
            "message_id": 500, "from": {"id": 6, "username": "bob", "first_name": "Bob"},
            "chat": {"id": chat, "type": "supergroup", "title": "Team"},
            "text": text, "message_thread_id": 9, "is_topic_message": true
        });
        if let Some(id) = reply_to {
            msg["reply_to_message"] = json!({"message_id": id});
        }
        msg
    }

    fn reply_to_question(
        reg: &mut Registry,
        chat: i64,
        text: &str,
        key: &str,
        q: usize,
    ) -> Vec<Effect> {
        let id = msg_id(reg, key, q);
        say(reg, text_msg(chat, text, Some(id)))
    }

    fn delivers(effects: &[Effect]) -> Vec<&Effect> {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::Deliver { .. }))
            .collect()
    }

    fn edits(effects: &[Effect]) -> Vec<&Effect> {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::Edit { .. }))
            .collect()
    }

    fn toast(effects: &[Effect]) -> String {
        match &effects[0] {
            Effect::AnswerCallback { text, .. } => text.clone(),
            other => panic!("first effect is not AnswerCallback: {other:?}"),
        }
    }

    #[test]
    fn single_select_delivers_answer() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, false)]);
        let effects = press(&mut reg, &key, 0, "o", Some(1));

        assert!(matches!(&effects[0], Effect::AnswerCallback { id, .. } if id == "cb1"));
        let delivered = delivers(&effects);
        assert_eq!(delivered.len(), 1, "{effects:?}");
        assert_eq!(
            delivered[0],
            &Effect::Deliver {
                window: WINDOW.into(),
                session_id: "s1".into(),
                request_id: 7,
                reply: json!({"kind": "answered", "answers": {"q1": ["b"]}}),
                by: "@alice".into(),
            }
        );
        // The final message is edited after the frontend reports success.
        let after = reg.delivered(&key, true);
        assert_eq!(after.len(), 1);
        match &after[0] {
            Effect::Edit {
                chat,
                message_id,
                html,
                keyboard,
                ..
            } => {
                assert_eq!(*chat, CHAT);
                assert_eq!(*message_id, 100);
                assert!(html.contains("Answered by @alice: Beta"), "{html}");
                assert!(keyboard.is_none());
            }
            other => panic!("{other:?}"),
        }
        assert!(reg.delivered(&key, true).is_empty());
    }

    #[test]
    fn multi_select_toggles_then_sends() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", true, false)]);
        let t = token(&reg, &key, 0);

        let first = tap(&mut reg, &format!("o:{t}:0"));
        assert!(delivers(&first).is_empty());
        match edits(&first)[0] {
            Effect::Edit { keyboard, .. } => {
                assert!(keyboard.as_ref().unwrap().to_string().contains("Alpha"));
            }
            _ => unreachable!(),
        }
        tap(&mut reg, &format!("o:{t}:2"));
        let untoggled = tap(&mut reg, &format!("o:{t}:0"));
        assert!(delivers(&untoggled).is_empty());

        let sent = tap(&mut reg, &format!("s:{t}"));
        let delivered = delivers(&sent);
        assert_eq!(delivered.len(), 1, "{sent:?}");
        match delivered[0] {
            Effect::Deliver { reply, .. } => {
                assert_eq!(
                    *reply,
                    json!({"kind": "answered", "answers": {"q1": ["c"]}})
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn multi_select_send_without_selection_is_refused() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", true, false)]);
        let effects = press(&mut reg, &key, 0, "s", None);
        assert_eq!(effects.len(), 1, "{effects:?}");
        assert!(toast(&effects).contains("at least one"));
        // The question is still open.
        let again = press(&mut reg, &key, 0, "o", Some(0));
        assert_eq!(edits(&again).len(), 1);
    }

    #[test]
    fn skip_one_of_two_questions_then_answer_other() {
        let mut reg = Registry::default();
        let key = add(
            &mut reg,
            "s1",
            7,
            vec![question("q1", false, false), question("q2", false, false)],
        );
        let skip = press(&mut reg, &key, 0, "k", None);
        assert!(delivers(&skip).is_empty());
        match edits(&skip)[0] {
            Effect::Edit {
                message_id,
                html,
                keyboard,
                ..
            } => {
                assert_eq!(*message_id, msg_id(&reg, &key, 0));
                assert!(html.contains("Skipped"), "{html}");
                assert!(keyboard.is_none());
            }
            _ => unreachable!(),
        }

        let answer = press(&mut reg, &key, 1, "o", Some(2));
        match delivers(&answer)[0] {
            Effect::Deliver { reply, .. } => {
                assert_eq!(
                    *reply,
                    json!({"kind": "answered", "answers": {"q2": ["c"]}})
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn all_skipped_delivers_skipped() {
        let mut reg = Registry::default();
        let key = add(
            &mut reg,
            "s1",
            7,
            vec![question("q1", false, false), question("q2", true, false)],
        );
        press(&mut reg, &key, 0, "k", None);
        let last = press(&mut reg, &key, 1, "k", None);
        match delivers(&last)[0] {
            Effect::Deliver { reply, .. } => assert_eq!(*reply, json!({"kind": "skipped"})),
            _ => unreachable!(),
        }
    }

    #[test]
    fn other_then_text_reply_delivers_custom() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, true)]);
        let ask = press(&mut reg, &key, 0, "c", None);
        assert!(matches!(&ask[0], Effect::AnswerCallback { .. }));
        assert!(
            ask.iter()
                .any(|e| matches!(e, Effect::Reply { chat, thread: Some(9), .. } if *chat == CHAT)),
            "{ask:?}"
        );
        assert!(reg.prompts[&key].awaiting_text[0]);

        let reply = reply_to_question(&mut reg, CHAT, "my own answer", &key, 0);
        let delivered = delivers(&reply);
        assert_eq!(delivered.len(), 1, "{reply:?}");
        match delivered[0] {
            Effect::Deliver { reply, by, .. } => {
                assert_eq!(
                    *reply,
                    json!({"kind": "answered",
                        "answers": {"q1": ["__custom__"]},
                        "custom": {"q1": "my own answer"}})
                );
                assert_eq!(by, "@bob");
            }
            _ => unreachable!(),
        }
        let after = reg.delivered(&key, true);
        match &after[0] {
            Effect::Edit { html, .. } => {
                assert!(html.contains("Answered by @bob: my own answer"), "{html}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_reply_without_other_tap_is_ignored() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, true)]);
        let effects = reply_to_question(&mut reg, CHAT, "hello", &key, 0);
        assert!(effects.is_empty(), "{effects:?}");
    }

    #[test]
    fn plain_topic_message_is_ignored() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, true)]);
        press(&mut reg, &key, 0, "c", None);
        // Every topic message replies to the topic creation message (id 9 here).
        let plain = say(&mut reg, text_msg(CHAT, "just chatting", Some(9)));
        assert!(plain.is_empty(), "{plain:?}");
        let no_reply = say(&mut reg, text_msg(CHAT, "no reply field", None));
        assert!(no_reply.is_empty(), "{no_reply:?}");
        assert!(reg.prompts[&key].awaiting_text[0]);
    }

    #[test]
    fn routes_by_session_and_request() {
        let mut reg = Registry::default();
        let key1 = add(&mut reg, "s1", 7, vec![question("q1", false, false)]);
        let key2 = add(&mut reg, "s2", 7, vec![question("q1", false, false)]);
        assert_ne!(key1, key2);

        let effects = press(&mut reg, &key2, 0, "o", Some(0));
        match delivers(&effects)[0] {
            Effect::Deliver {
                session_id,
                request_id,
                ..
            } => {
                assert_eq!(session_id, "s2");
                assert_eq!(*request_id, 7);
            }
            _ => unreachable!(),
        }
        // The other prompt is untouched and still answerable.
        assert!(!reg.prompts[&key1].done[0]);
        let other = press(&mut reg, &key1, 0, "o", Some(1));
        match delivers(&other)[0] {
            Effect::Deliver { session_id, .. } => assert_eq!(session_id, "s1"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn stale_token_reports_expired() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, false)]);
        let unknown = tap(&mut reg, "o:nope000000:0");
        assert_eq!(unknown.len(), 1);
        assert_eq!(toast(&unknown), "This question has expired");

        // Tapping a question that was already answered.
        let t = token(&reg, &key, 0);
        tap(&mut reg, &format!("o:{t}:0"));
        let again = tap(&mut reg, &format!("o:{t}:1"));
        assert_eq!(again.len(), 1);
        assert_eq!(toast(&again), "This question has expired");

        // Tapping after the desktop closed the prompt.
        let key2 = add(&mut reg, "s2", 8, vec![question("q1", false, false)]);
        let t2 = token(&reg, &key2, 0);
        reg.close("s2", 8, "Answered on desktop");
        let closed = tap(&mut reg, &format!("o:{t2}:0"));
        assert_eq!(closed.len(), 1);
        assert_eq!(toast(&closed), "This question has expired");
        assert!(delivers(&closed).is_empty());

        // Malformed data never panics.
        assert_eq!(toast(&tap(&mut reg, "")), "This question has expired");
        assert_eq!(toast(&tap(&mut reg, "o")), "This question has expired");
    }

    #[test]
    fn out_of_range_option_is_refused() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, false)]);
        let effects = press(&mut reg, &key, 0, "o", Some(99));
        assert_eq!(effects.len(), 1, "{effects:?}");
        assert!(!reg.prompts[&key].done[0]);
    }

    #[test]
    fn unconfigured_chat_ignored() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, true)]);
        let t = token(&reg, &key, 0);
        assert!(tap_in(&mut reg, -999, &format!("o:{t}:0")).is_empty());
        assert!(!reg.prompts[&key].done[0]);

        reg.prompts.get_mut(&key).unwrap().awaiting_text[0] = true;
        let stray = reply_to_question(&mut reg, -999, "x", &key, 0);
        assert!(stray.is_empty());

        let mut codes = Codes::default();
        let code = codes.issue(
            CodePurpose::Link {
                project: "p".into(),
                session_id: "s".into(),
            },
            0,
        );
        let link = say_with(
            &mut reg,
            text_msg(-999, &format!("/link {code}"), None),
            &mut codes,
        );
        assert!(link.is_empty(), "{link:?}");
        // The code was not consumed by the stranger.
        assert!(codes.take(&code, 0).is_some());
    }

    fn connect_msg(chat: i64, text: &str) -> Value {
        json!({"message_id": 1, "from": {"id": 6, "username": "bob"},
            "chat": {"id": chat, "type": "supergroup", "title": "Team"}, "text": text})
    }

    #[test]
    fn connect_code_connects() {
        let mut reg = Registry::default();
        let mut codes = Codes::default();
        let code = codes.issue(
            CodePurpose::Connect {
                project: "proj".into(),
            },
            0,
        );

        // Unconfigured chat, addressed command.
        let effects = say_with(
            &mut reg,
            connect_msg(-777, &format!("/start@my_bot {code}")),
            &mut codes,
        );
        assert_eq!(
            effects,
            vec![Effect::Connected {
                project: "proj".into(),
                chat: -777,
                title: "Team".into()
            }]
        );
        // The code works once.
        let again = say_with(
            &mut reg,
            connect_msg(-777, &format!("/start@my_bot {code}")),
            &mut codes,
        );
        assert!(again.is_empty(), "{again:?}");

        // /connect works too, bare and lower-case code.
        let code2 = codes.issue(
            CodePurpose::Connect {
                project: "proj".into(),
            },
            0,
        );
        let effects = say_with(
            &mut reg,
            connect_msg(-778, &format!("/connect {}", code2.to_lowercase())),
            &mut codes,
        );
        assert!(matches!(&effects[0], Effect::Connected { chat: -778, .. }));
    }

    #[test]
    fn wrong_code_or_purpose_does_not_connect() {
        let mut reg = Registry::default();
        let mut codes = Codes::default();
        let bad = say_with(
            &mut reg,
            connect_msg(-777, "/start@my_bot ZZZZZZZZ"),
            &mut codes,
        );
        assert!(bad.is_empty());
        let no_code = say_with(&mut reg, connect_msg(-777, "/start"), &mut codes);
        assert!(no_code.is_empty());
        let link_code = codes.issue(
            CodePurpose::Link {
                project: "p".into(),
                session_id: "s".into(),
            },
            0,
        );
        let wrong = say_with(
            &mut reg,
            connect_msg(-777, &format!("/connect {link_code}")),
            &mut codes,
        );
        assert!(wrong.is_empty());
        // A private chat is never a group to connect.
        let code = codes.issue(
            CodePurpose::Connect {
                project: "proj".into(),
            },
            0,
        );
        let mut private = connect_msg(42, &format!("/start {code}"));
        private["chat"]["type"] = json!("private");
        assert!(say_with(&mut reg, private, &mut codes).is_empty());
    }

    #[test]
    fn link_in_topic_links() {
        let mut reg = Registry::default();
        let mut codes = Codes::default();
        let code = codes.issue(
            CodePurpose::Link {
                project: "proj".into(),
                session_id: "s1".into(),
            },
            0,
        );
        let effects = say_with(
            &mut reg,
            text_msg(CHAT, &format!("/link@my_bot {code}"), Some(9)),
            &mut codes,
        );
        assert_eq!(
            effects,
            vec![Effect::Linked {
                project: "proj".into(),
                session_id: "s1".into(),
                thread: 9
            }]
        );
    }

    #[test]
    fn link_in_general_hints() {
        let mut reg = Registry::default();
        let mut codes = Codes::default();
        let code = codes.issue(
            CodePurpose::Link {
                project: "proj".into(),
                session_id: "s1".into(),
            },
            0,
        );
        let mut msg = connect_msg(CHAT, &format!("/link@my_bot {code}"));
        msg["chat"]["is_forum"] = json!(true);
        let effects = say_with(&mut reg, msg, &mut codes);
        assert_eq!(effects.len(), 1, "{effects:?}");
        match &effects[0] {
            Effect::Reply {
                chat, thread, html, ..
            } => {
                assert_eq!(*chat, CHAT);
                assert_eq!(*thread, None);
                assert!(html.contains("topic"), "{html}");
            }
            other => panic!("{other:?}"),
        }
        // The hint does not burn the code.
        assert!(codes.take(&code, 0).is_some());
    }

    #[test]
    fn service_message_migration_effect() {
        let mut reg = Registry::default();
        let new_chat = -1002_i64;
        let service = |chat: i64, key: &str, other: i64| json!({"message_id": 3, "chat": {"id": chat, "type": "group"}, key: other});
        let effects = say(&mut reg, service(CHAT, "migrate_to_chat_id", new_chat));
        assert_eq!(
            effects,
            [Effect::Migrated {
                from: CHAT,
                to: new_chat
            }]
        );
        // The matching message in the new supergroup names the configured old chat.
        let effects = say(&mut reg, service(new_chat, "migrate_from_chat_id", CHAT));
        assert_eq!(
            effects,
            [Effect::Migrated {
                from: CHAT,
                to: new_chat
            }]
        );
        // A group that is not configured cannot move anything.
        assert!(say(&mut reg, service(-999, "migrate_to_chat_id", new_chat)).is_empty());
        assert!(say(&mut reg, service(new_chat, "migrate_from_chat_id", -999)).is_empty());
        assert!(say(&mut reg, service(CHAT, "migrate_to_chat_id", CHAT)).is_empty());
    }

    #[test]
    fn expire_edits_every_message_and_drops_tokens() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, false)]);
        let t = token(&reg, &key, 0);
        let effects = reg.expire(&key);
        assert_eq!(effects.len(), 1, "{effects:?}");
        assert!(
            matches!(&effects[0], Effect::Edit { html, keyboard: None, .. } if html.contains("This question has expired"))
        );
        assert_eq!(
            toast(&tap(&mut reg, &format!("o:{t}:0"))),
            "This question has expired"
        );
        assert!(reg.expire(&key).is_empty());
    }

    #[test]
    fn is_same_prompt_compares_owner_and_shape() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, false)]);
        let same = payload("s1", 7, vec![question("q1", false, false)]);
        assert!(reg.is_same_prompt(&key, WINDOW, &same));
        assert!(!reg.is_same_prompt(&key, "other", &same));
        let other_question = payload("s1", 7, vec![question("q2", false, false)]);
        assert!(!reg.is_same_prompt(&key, WINDOW, &other_question));
        let mut other_option = same.clone();
        other_option.questions[0].options[0].id = "zzz".into();
        assert!(!reg.is_same_prompt(&key, WINDOW, &other_option));
        assert!(!reg.is_same_prompt("missing#1", WINDOW, &same));
    }

    #[test]
    fn close_edits_every_message_of_prompt() {
        let mut reg = Registry::default();
        let key = add(
            &mut reg,
            "s1",
            7,
            vec![question("q1", false, false), question("q2", true, false)],
        );
        // One question already answered on Telegram: still edited, desktop won the prompt.
        press(&mut reg, &key, 0, "o", Some(0));
        let ids: Vec<i64> = (0..2).map(|i| msg_id(&reg, &key, i)).collect();
        let effects = reg.close("s1", 7, "Answered on desktop");
        assert_eq!(effects.len(), 2, "{effects:?}");
        for (i, effect) in effects.iter().enumerate() {
            match effect {
                Effect::Edit {
                    chat,
                    message_id,
                    html,
                    keyboard,
                    ..
                } => {
                    assert_eq!(*chat, CHAT);
                    assert_eq!(*message_id, ids[i]);
                    assert!(html.contains("Answered on desktop"), "{html}");
                    assert!(keyboard.is_none());
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(reg.close("s1", 7, "Answered on desktop").is_empty());
    }

    #[test]
    fn delivery_failure_marks_every_message_expired() {
        let mut reg = Registry::default();
        let key = add(
            &mut reg,
            "s1",
            7,
            vec![question("q1", false, false), question("q2", false, false)],
        );
        press(&mut reg, &key, 0, "o", Some(0));
        let last = press(&mut reg, &key, 1, "o", Some(0));
        assert_eq!(delivers(&last).len(), 1);

        let failed = reg.delivered(&key, false);
        assert_eq!(failed.len(), 2, "{failed:?}");
        for effect in &failed {
            match effect {
                Effect::Edit { html, keyboard, .. } => {
                    assert!(html.contains("This question has expired"), "{html}");
                    assert!(keyboard.is_none());
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(reg.prompts.is_empty());
    }

    #[test]
    fn desktop_close_while_delivering_is_deferred_to_the_result() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, false)]);
        press(&mut reg, &key, 0, "o", Some(0));
        // The frontend clearing its pendingQuestion looks like a desktop answer.
        assert!(reg.close("s1", 7, "Answered on desktop").is_empty());
        let ok = reg.delivered(&key, true);
        match &ok[0] {
            Effect::Edit { html, .. } => {
                assert!(html.contains("Answered by @alice: Alpha"), "{html}")
            }
            other => panic!("{other:?}"),
        }

        // If the delivery then fails, the desktop status explains why.
        let key2 = add(&mut reg, "s2", 8, vec![question("q1", false, false)]);
        press(&mut reg, &key2, 0, "o", Some(0));
        reg.close("s2", 8, "Answered on desktop");
        let failed = reg.delivered(&key2, false);
        match &failed[0] {
            Effect::Edit { html, .. } => assert!(html.contains("Answered on desktop"), "{html}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_option_label_asks_for_text() {
        let mut reg = Registry::default();
        let mut q = question("q1", false, true);
        q.options.push(option("z", "Other"));
        let key = add(&mut reg, "s1", 7, vec![q]);
        let effects = press(&mut reg, &key, 0, "o", Some(3));
        assert!(delivers(&effects).is_empty(), "{effects:?}");
        assert!(reg.prompts[&key].awaiting_text[0]);
    }

    #[test]
    fn real_other_option_sends_its_own_id_with_the_text() {
        let mut reg = Registry::default();
        let mut q = question("q1", false, true);
        q.options.push(option("z", "Other"));
        let key = add(&mut reg, "s1", 7, vec![q]);
        press(&mut reg, &key, 0, "o", Some(3));
        let reply = reply_to_question(&mut reg, CHAT, "my own answer", &key, 0);
        match delivers(&reply)[0] {
            Effect::Deliver { reply, .. } => assert_eq!(
                *reply,
                json!({"kind": "answered",
                    "answers": {"q1": ["z"]},
                    "custom": {"q1": "my own answer"}})
            ),
            _ => unreachable!(),
        }
        let after = reg.delivered(&key, true);
        match &after[0] {
            Effect::Edit { html, .. } => {
                assert!(html.contains("Answered by @bob: my own answer"), "{html}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn real_other_option_in_multi_select_keeps_other_picks() {
        let mut reg = Registry::default();
        let mut q = question("q1", true, true);
        q.options.push(option("z", "other "));
        let key = add(&mut reg, "s1", 7, vec![q]);
        let t = token(&reg, &key, 0);
        tap(&mut reg, &format!("o:{t}:1"));
        tap(&mut reg, &format!("o:{t}:3"));
        let added = reply_to_question(&mut reg, CHAT, "extra", &key, 0);
        assert!(delivers(&added).is_empty(), "{added:?}");
        let sent = tap(&mut reg, &format!("s:{t}"));
        match delivers(&sent)[0] {
            Effect::Deliver { reply, .. } => assert_eq!(
                *reply,
                json!({"kind": "answered",
                    "answers": {"q1": ["b", "z"]},
                    "custom": {"q1": "extra"}})
            ),
            _ => unreachable!(),
        }
    }

    #[test]
    fn multi_select_custom_text_is_added_then_sent() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", true, true)]);
        let t = token(&reg, &key, 0);
        tap(&mut reg, &format!("o:{t}:1"));
        tap(&mut reg, &format!("c:{t}"));
        let added = reply_to_question(&mut reg, CHAT, "extra", &key, 0);
        assert!(delivers(&added).is_empty(), "{added:?}");
        assert_eq!(edits(&added).len(), 1);
        let sent = tap(&mut reg, &format!("s:{t}"));
        match delivers(&sent)[0] {
            Effect::Deliver { reply, .. } => assert_eq!(
                *reply,
                json!({"kind": "answered",
                    "answers": {"q1": ["b", "__custom__"]},
                    "custom": {"q1": "extra"}})
            ),
            _ => unreachable!(),
        }
    }

    fn code_for(codes: &mut Codes, purpose: CodePurpose) -> String {
        codes.issue(purpose, 0)
    }

    fn connect_for(project: &str) -> CodePurpose {
        CodePurpose::Connect {
            project: project.into(),
        }
    }

    fn link_for(project: &str) -> CodePurpose {
        CodePurpose::Link {
            project: project.into(),
            session_id: "s1".into(),
        }
    }

    #[test]
    fn command_addressed_to_another_bot_is_ignored_and_keeps_the_code() {
        let mut reg = Registry::default();
        let mut codes = Codes::default();
        let connect_code = code_for(&mut codes, connect_for("proj"));
        let link_code = code_for(&mut codes, link_for("proj"));
        for text in [
            format!("/start@other_bot {connect_code}"),
            format!("/connect@Other_Bot {connect_code}"),
        ] {
            // Configured chat, so a wrong code would be answered if the command counted.
            let effects = say_with(&mut reg, connect_msg(CHAT, &text), &mut codes);
            assert!(effects.is_empty(), "{text}: {effects:?}");
        }
        let effects = say_with(
            &mut reg,
            text_msg(CHAT, &format!("/link@other_bot {link_code}"), Some(9)),
            &mut codes,
        );
        assert!(effects.is_empty(), "{effects:?}");
        // Neither code was burned.
        assert!(codes.take(&connect_code, 0).is_some());
        assert!(codes.take(&link_code, 0).is_some());
    }

    #[test]
    fn bare_and_own_commands_still_work() {
        let mut reg = Registry::default();
        let mut codes = Codes::default();
        let code = code_for(&mut codes, connect_for("proj"));
        let own = say_with(
            &mut reg,
            connect_msg(-777, &format!("/start@MY_BOT {code}")),
            &mut codes,
        );
        assert!(matches!(&own[..], [Effect::Connected { .. }]), "{own:?}");
        let code = code_for(&mut codes, link_for("proj"));
        let bare = say_with(
            &mut reg,
            text_msg(CHAT, &format!("/link {code}"), Some(9)),
            &mut codes,
        );
        assert!(matches!(&bare[..], [Effect::Linked { .. }]), "{bare:?}");
    }

    #[test]
    fn code_of_a_project_of_another_bot_is_left_alone() {
        let mut reg = Registry::default();
        let mut codes = Codes::default();
        let connect_code = code_for(&mut codes, connect_for("elsewhere"));
        let link_code = code_for(&mut codes, link_for("elsewhere"));
        // Bare commands reach every bot in the group; this bot serves "proj" only.
        let connect = say_with(
            &mut reg,
            connect_msg(CHAT, &format!("/connect {connect_code}")),
            &mut codes,
        );
        assert!(connect.is_empty(), "{connect:?}");
        let link = say_with(
            &mut reg,
            text_msg(CHAT, &format!("/link {link_code}"), Some(9)),
            &mut codes,
        );
        assert!(link.is_empty(), "{link:?}");
        assert_eq!(
            codes.take(&connect_code, 0),
            Some(connect_for("elsewhere")),
            "connect code must survive"
        );
        assert_eq!(
            codes.take(&link_code, 0),
            Some(link_for("elsewhere")),
            "link code must survive"
        );
    }

    #[test]
    fn reply_to_another_bots_question_is_ignored() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, true)]);
        reg.prompts.get_mut(&key).unwrap().bot = "other_bot".into();
        press(&mut reg, &key, 0, "c", None);
        let id = msg_id(&reg, &key, 0);

        let wrong = say_with(
            &mut reg,
            text_msg(CHAT, "answer", Some(id)),
            &mut Codes::default(),
        );
        assert!(wrong.is_empty(), "{wrong:?}");
        assert!(reg.prompts[&key].awaiting_text[0], "still waiting");
        assert!(!reg.prompts[&key].done[0]);

        let right = say_as(
            &mut reg,
            text_msg(CHAT, "answer", Some(id)),
            &mut Codes::default(),
            "other_bot",
        );
        assert_eq!(delivers(&right).len(), 1, "{right:?}");
    }

    #[test]
    fn effects_of_a_prompt_carry_its_project() {
        let mut reg = Registry::default();
        let key = add(&mut reg, "s1", 7, vec![question("q1", false, true)]);
        let ask = press(&mut reg, &key, 0, "c", None);
        assert!(
            ask.iter().any(
                |e| matches!(e, Effect::Reply { project: Some(project), .. } if project == "proj")
            ),
            "{ask:?}"
        );
        let answered = reply_to_question(&mut reg, CHAT, "text", &key, 0);
        let after = reg.delivered(&key, true);
        for effect in answered.iter().chain(after.iter()) {
            if let Effect::Edit { project, .. } = effect {
                assert_eq!(project, "proj");
            }
        }
        assert!(matches!(&after[0], Effect::Edit { project, .. } if project == "proj"));
        // A reply to a command has no prompt, so no project.
        let invalid = say(&mut reg, connect_msg(CHAT, "/start nope"));
        assert!(
            matches!(&invalid[..], [Effect::Reply { project: None, .. }]),
            "{invalid:?}"
        );
    }
}
