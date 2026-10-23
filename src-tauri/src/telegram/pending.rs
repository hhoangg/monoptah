//! Registry of questions that were forwarded to Telegram and are still waiting.
//!
//! Pure state machine: no I/O, no threads, time is passed in. Updates from Telegram go in
//! through `handle_update` and come out as `Effect`s the caller performs. Text from
//! Telegram is data only: it can become a custom answer string and nothing else.
//!
//! Commands (`/start`, `/connect`, `/link`) are matched leniently: the `@bot` suffix is
//! ignored. The codes are random, single-use and expire, so a command that reaches the
//! wrong bot cannot do harm, and the registry needs no bot username.
// Consumed by Task 5, where this allow is removed.
#![allow(dead_code)]

use super::config::{CodePurpose, Codes};
use super::render::{
    is_other, keyboard, new_token, question_html, status_html, ForwardPayload, CUSTOM_OPTION_ID,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub struct PendingPrompt {
    pub owner_window: String,
    pub payload: ForwardPayload,
    pub chat: i64,
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
    pub fn new(owner_window: String, payload: ForwardPayload, chat: i64) -> Self {
        let count = payload.questions.len();
        Self {
            owner_window,
            payload,
            chat,
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
        chat: i64,
        thread: Option<i64>,
        html: String,
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

#[derive(Default)]
pub struct Registry {
    /// "<sessionId>#<requestId>" -> prompt
    prompts: BTreeMap<String, PendingPrompt>,
    /// token -> (key, question index); only for questions that are not done
    tokens: BTreeMap<String, (String, usize)>,
    delivering: BTreeMap<String, Delivering>,
}

const EXPIRED: &str = "This question has expired";
const MAX_CUSTOM_CHARS: usize = 2000;

fn key_of(session_id: &str, request_id: u64) -> String {
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

/// `/name@bot arg` -> ("name", "arg"). The bot suffix is ignored, see the module docs.
fn parse_command(text: &str) -> Option<(String, &str)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (head, arg) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(head, arg)| (head, arg.trim()));
    let name = head.split('@').next().unwrap_or("").to_ascii_lowercase();
    Some((name, arg))
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

/// Option ids in option order, then the custom marker.
fn selected_ids(prompt: &PendingPrompt, q_index: usize) -> Vec<String> {
    let selected = &prompt.selected[q_index];
    let mut ids: Vec<String> = prompt.payload.questions[q_index]
        .options
        .iter()
        .filter(|option| selected.contains(&option.id))
        .map(|option| option.id.clone())
        .collect();
    if selected.contains(CUSTOM_OPTION_ID) {
        ids.push(CUSTOM_OPTION_ID.to_string());
    }
    ids
}

fn selected_labels(prompt: &PendingPrompt, q_index: usize) -> Vec<String> {
    let q = &prompt.payload.questions[q_index];
    selected_ids(prompt, q_index)
        .iter()
        .filter_map(|id| {
            if id == CUSTOM_OPTION_ID {
                return prompt.custom[q_index].clone();
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
        if ids.iter().any(|id| id == CUSTOM_OPTION_ID) {
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

    pub fn set_message(&mut self, key: &str, q_index: usize, message_id: i64) {
        if let Some(slot) = self
            .prompts
            .get_mut(key)
            .and_then(|prompt| prompt.messages.get_mut(q_index))
        {
            *slot = Some(message_id);
        }
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
        codes: &mut Codes,
        now_ms: u64,
    ) -> Vec<Effect> {
        if let Some(callback) = update.get("callback_query") {
            return self.handle_callback(callback, configured_chats);
        }
        if let Some(message) = update.get("message") {
            return self.handle_message(message, configured_chats, codes, now_ms);
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
        codes: &mut Codes,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(chat) = msg.pointer("/chat/id").and_then(Value::as_i64) else {
            return Vec::new();
        };
        if msg.pointer("/from/is_bot").and_then(Value::as_bool) == Some(true) {
            return Vec::new();
        }
        let text = msg["text"].as_str().unwrap_or("");
        let configured = chats.contains(&chat);
        match parse_command(text) {
            Some((name, arg)) if name == "start" || name == "connect" => {
                return connect(msg, chat, configured, arg, codes, now_ms);
            }
            Some((name, arg)) if name == "link" => {
                return link(msg, chat, configured, arg, codes, now_ms);
            }
            _ => {}
        }
        if !configured {
            return Vec::new();
        }
        self.handle_text(msg, chat, text)
    }

    /// A reply to a question message the user asked to answer with text. Only a reply to
    /// exactly that message counts: topic messages all carry a `reply_to_message` that
    /// points at the topic creation message, and those must not become answers.
    fn handle_text(&mut self, msg: &Value, chat: i64, text: &str) -> Vec<Effect> {
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
            if prompt.chat != chat {
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
        if prompt.payload.questions[q_index].multi_select {
            prompt.selected[q_index].insert(CUSTOM_OPTION_ID.to_string());
            return edit_open(prompt, q_index).into_iter().collect();
        }
        prompt.selected[q_index] = BTreeSet::from([CUSTOM_OPTION_ID.to_string()]);
        self.finish_question(&key, q_index, &by)
    }
}

fn reply(chat: i64, thread: Option<i64>, html: &str) -> Effect {
    Effect::Reply {
        chat,
        thread,
        html: html.to_string(),
    }
}

/// `/start <code>` or `/connect <code>` in a group: the one place an unconfigured chat is
/// listened to. A bad code is answered only in an already configured chat.
fn connect(
    msg: &Value,
    chat: i64,
    configured: bool,
    code: &str,
    codes: &mut Codes,
    now_ms: u64,
) -> Vec<Effect> {
    if code.is_empty() || !is_group(&msg["chat"]) {
        return Vec::new();
    }
    match codes.take(code, now_ms) {
        Some(CodePurpose::Connect { project }) => vec![Effect::Connected {
            project,
            chat,
            title: msg
                .pointer("/chat/title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }],
        _ if configured => vec![reply(
            chat,
            thread_of(msg),
            "That code is not valid or has expired.",
        )],
        _ => Vec::new(),
    }
}

/// `/link <code>` inside a topic of a configured group.
fn link(
    msg: &Value,
    chat: i64,
    configured: bool,
    code: &str,
    codes: &mut Codes,
    now_ms: u64,
) -> Vec<Effect> {
    if !configured {
        return Vec::new();
    }
    let Some(thread) = thread_of(msg) else {
        return vec![reply(
            chat,
            None,
            "Send /link inside the topic you want to link, not in General.",
        )];
    };
    match codes.take(code, now_ms) {
        Some(CodePurpose::Link {
            project,
            session_id,
        }) => vec![Effect::Linked {
            project,
            session_id,
            thread,
        }],
        _ => vec![reply(
            chat,
            Some(thread),
            "That code is not valid or has expired.",
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::super::render::{OptionIn, QuestionIn};
    use super::*;

    const CHAT: i64 = -1001;
    const WINDOW: &str = "main";

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
        let prompt = PendingPrompt::new(WINDOW.into(), payload(session, request_id, qs), CHAT);
        let key = reg.insert(prompt);
        let base = 100 * (reg.prompts.len() as i64);
        for i in 0..count {
            reg.set_message(&key, i, base + i as i64);
        }
        key
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
        reg.handle_update(&update, &chats(), &mut Codes::default(), 0)
    }

    fn say(reg: &mut Registry, msg: Value) -> Vec<Effect> {
        say_with(reg, msg, &mut Codes::default())
    }

    fn say_with(reg: &mut Registry, msg: Value, codes: &mut Codes) -> Vec<Effect> {
        reg.handle_update(&json!({"update_id": 2, "message": msg}), &chats(), codes, 0)
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
            Effect::Reply { chat, thread, html } => {
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
}
