//! The state machine of `src/features/telegram/hooks/useTelegramQuestions.ts` as a plain
//! struct. The shell owns the clock, the Telegram status reads and the sessions; it feeds
//! snapshots and events in and carries out the returned actions:
//!
//! 1. every 15 s (`TICK_MS`), and whenever a session's pending question changes, call
//!    `sync`, then read the statuses `projects_to_read` names (through a `StatusCache`, off
//!    the UI thread) and call `forward`;
//! 2. `Action::Forward` -> `telegram::forward_question` off the UI thread, then
//!    `forward_done` with its result;
//! 3. `Action::Close` -> `telegram::question_closed`;
//! 4. `Action::Apply` -> `Conversation::answer_question`, then `answer_applied`;
//! 5. `Action::AnswerResult` -> `telegram::answer_result`;
//! 6. a desktop answer, skip or cancel -> `record_outcome`, before the snapshot stops
//!    listing the question.
//!
//! Sessions without a pending question may be left out of the snapshots.
use super::forward::{
    due_forwards, forward_delay_ms, forward_payload, is_forwardable, project_key, question_key,
    Pending, SessionInfo, FORWARD_DELAY_MS,
};
use super::poller::TelegramStatus;
use super::render::{is_other, ForwardPayload, CUSTOM_OPTION_ID};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const TICK_MS: u64 = 15_000;
pub const MAX_FORWARD_ATTEMPTS: u32 = 2;
/// How long a project's status is reused before it is read again.
pub const STATUS_TTL_MS: u64 = 60_000;
/// Desktop outcomes remembered for questions that were not closed yet.
const MAX_OUTCOMES: usize = 200;

/// How a question ended on the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Answered,
    Skipped,
    Cancelled,
}

impl Outcome {
    /// The line `telegram::question_closed` puts on the Telegram message.
    pub fn closed_status(self) -> &'static str {
        match self {
            Self::Answered => "Answered on desktop",
            Self::Skipped => "Skipped",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// What the forwarder needs to know about a project's Telegram setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectStatus {
    pub configured: bool,
    pub enabled: bool,
    pub has_chat: bool,
    pub delay_secs: u64,
}

impl ProjectStatus {
    pub fn can_forward(&self) -> bool {
        self.configured && self.enabled && self.has_chat
    }
}

impl From<&TelegramStatus> for ProjectStatus {
    fn from(status: &TelegramStatus) -> Self {
        Self {
            configured: status.configured,
            enabled: status.enabled,
            has_chat: status.chat_id.is_some(),
            delay_secs: status.delay_secs,
        }
    }
}

/// Successful status reads, kept for a minute. A failed read is never stored (just do not
/// `put`), so the next caller reads again.
#[derive(Default)]
pub struct StatusCache {
    entries: HashMap<String, (u64, ProjectStatus)>,
}

impl StatusCache {
    pub fn get(&self, project: &str, now: u64) -> Option<&ProjectStatus> {
        self.entries
            .get(project)
            .filter(|(at, _)| now.saturating_sub(*at) < STATUS_TTL_MS)
            .map(|(_, status)| status)
    }

    pub fn put(&mut self, project: &str, now: u64, status: ProjectStatus) {
        self.entries.insert(project.to_string(), (now, status));
    }

    /// Drops one project, or all of them; call it when the project's settings change.
    pub fn invalidate(&mut self, project: Option<&str>) {
        match project {
            Some(project) => {
                self.entries.remove(project);
            }
            None => self.entries.clear(),
        }
    }
}

/// An answer a Telegram user gave, as the bridge hands it to the shell.
#[derive(Clone, Debug, PartialEq)]
pub struct AnswerEvent {
    /// Pass back to `telegram::answer_result`.
    pub key: String,
    pub session_id: String,
    pub request_id: u64,
    pub reply: Reply,
    pub by: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Skipped,
    Answered {
        /// Question id -> chosen option ids (`__custom__` for typed text).
        answers: BTreeMap<String, Vec<String>>,
        /// Question id -> typed text.
        custom: BTreeMap<String, String>,
    },
}

impl AnswerEvent {
    /// Reads the `monocode-telegram-answer` payload `{key, sessionId, requestId, reply, by}`.
    pub fn from_json(value: &Value) -> Option<Self> {
        let reply = value.get("reply")?;
        let reply = match reply.get("kind")?.as_str()? {
            "skipped" => Reply::Skipped,
            "answered" => {
                let mut answers = BTreeMap::new();
                for (question, ids) in reply.get("answers")?.as_object()? {
                    let ids = ids.as_array()?;
                    let ids = ids.iter().map(|id| id.as_str().map(str::to_string));
                    answers.insert(question.clone(), ids.collect::<Option<Vec<_>>>()?);
                }
                let mut custom = BTreeMap::new();
                if let Some(typed) = reply.get("custom") {
                    for (question, text) in typed.as_object()? {
                        custom.insert(question.clone(), text.as_str()?.to_string());
                    }
                }
                Reply::Answered { answers, custom }
            }
            _ => return None,
        };
        Some(Self {
            key: value.get("key")?.as_str()?.to_string(),
            session_id: value.get("sessionId")?.as_str()?.to_string(),
            request_id: value.get("requestId")?.as_u64()?,
            reply,
            by: value
                .get("by")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }
}

/// A Telegram answer to apply to the session's question.
#[derive(Clone, Debug, PartialEq)]
pub struct ApplyAnswer {
    /// Pass `answer_applied` this same value.
    pub result_key: String,
    pub session_id: String,
    pub request_id: u64,
    /// The `Pending::id` of the question.
    pub question_id: String,
    /// Native answers: one list of option labels or typed text per question, in order;
    /// `None` skips the question.
    pub answers: Option<Vec<Vec<String>>>,
}

#[derive(Debug)]
pub enum Action {
    /// Send the question to Telegram, then report with `forward_done`.
    Forward(ForwardPayload),
    /// Tell Telegram how the desktop settled a question it was sent.
    Close {
        session_id: String,
        request_id: u64,
        status: &'static str,
    },
    /// Apply a Telegram answer, then report with `answer_applied`.
    Apply(ApplyAnswer),
    /// The verdict on an answer event, for `telegram::answer_result`.
    AnswerResult { key: String, ok: bool },
}

/// Whether a question may ever be forwarded, decided when it is first seen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Eligibility {
    Checking,
    Yes,
    No,
}

#[derive(Default)]
pub struct Forwarder {
    first_seen: HashMap<String, u64>,
    eligibility: HashMap<String, Eligibility>,
    sent: HashSet<String>,
    /// Session id and request id of every sent question.
    sent_info: HashMap<String, (String, u64)>,
    in_flight: HashSet<String>,
    attempts: HashMap<String, u32>,
    /// Questions Telegram answered, so their closing is not reported back.
    by_telegram: HashSet<String>,
    /// Wait in ms per project, from the last status that was read.
    delays: HashMap<String, u64>,
    /// Oldest first.
    outcomes: Vec<(String, Outcome)>,
}

impl Forwarder {
    /// Notes how the desktop settled a question. Call it as soon as it happens, before the
    /// snapshots stop listing the question.
    pub fn record_outcome(&mut self, session_id: &str, request_id: u64, outcome: Outcome) {
        let key = question_key(session_id, request_id);
        self.outcomes.retain(|(k, _)| *k != key);
        self.outcomes.push((key, outcome));
        if self.outcomes.len() > MAX_OUTCOMES {
            self.outcomes.remove(0);
        }
    }

    pub fn peek_outcome(&self, session_id: &str, request_id: u64) -> Option<Outcome> {
        let key = question_key(session_id, request_id);
        self.outcomes
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, o)| *o)
    }

    fn take_outcome(&mut self, key: &str) -> Option<Outcome> {
        let at = self.outcomes.iter().position(|(k, _)| k == key)?;
        Some(self.outcomes.remove(at).1)
    }

    /// The projects whose status `forward` needs now: those with a question still being
    /// checked or waiting to be sent.
    pub fn projects_to_read(&self, sessions: &[SessionInfo]) -> Vec<String> {
        let mut projects = BTreeSet::new();
        for session in sessions {
            let Some(pending) = &session.pending else {
                continue;
            };
            let key = question_key(&session.id, pending.request_id);
            let wanted = match self.eligibility.get(&key) {
                Some(Eligibility::Checking) => true,
                Some(Eligibility::Yes) => self.can_try(&key),
                _ => false,
            };
            if wanted {
                projects.insert(project_key(&session.cwd));
            }
        }
        projects.into_iter().collect()
    }

    fn can_try(&self, key: &str) -> bool {
        !self.sent.contains(key)
            && !self.in_flight.contains(key)
            && self.attempts.get(key).copied().unwrap_or(0) < MAX_FORWARD_ATTEMPTS
    }

    /// Sees new questions, reports closed ones and forgets finished ones.
    pub fn sync(&mut self, now: u64, sessions: &[SessionInfo]) -> Vec<Action> {
        let mut pending = HashSet::new();
        for session in sessions {
            let Some(question) = &session.pending else {
                continue;
            };
            let key = question_key(&session.id, question.request_id);
            pending.insert(key.clone());
            if self.eligibility.contains_key(&key) {
                continue;
            }
            // Request ids repeat, so an outcome stored for this key belongs to an earlier
            // question and must not describe this one.
            self.take_outcome(&key);
            // Decided once, when first seen: a later change must not flip it. Questions
            // with an auto-skip deadline never block the agent.
            if question.auto_resolve || !is_forwardable(session) {
                self.eligibility.insert(key, Eligibility::No);
                continue;
            }
            self.eligibility.insert(key.clone(), Eligibility::Checking);
            self.first_seen.insert(key, now);
        }

        let mut actions = Vec::new();
        let mut sent: Vec<String> = self.sent.iter().cloned().collect();
        sent.sort();
        for key in sent {
            if pending.contains(&key) {
                continue;
            }
            let Some((session_id, request_id)) = self.sent_info.remove(&key) else {
                continue;
            };
            self.sent.remove(&key);
            let outcome = self.take_outcome(&key).unwrap_or(Outcome::Cancelled);
            if self.by_telegram.remove(&key) {
                continue;
            }
            actions.push(Action::Close {
                session_id,
                request_id,
                status: outcome.closed_status(),
            });
        }

        let gone: Vec<String> = self
            .eligibility
            .keys()
            .filter(|key| !pending.contains(*key) && !self.in_flight.contains(*key))
            .cloned()
            .collect();
        for key in gone {
            self.eligibility.remove(&key);
            self.first_seen.remove(&key);
            self.attempts.remove(&key);
        }
        actions
    }

    /// Decides the eligibility of questions being checked and returns the questions that
    /// are due. A project missing from `statuses` (its read failed) decides nothing and
    /// costs no attempt.
    pub fn forward(
        &mut self,
        now: u64,
        sessions: &[SessionInfo],
        statuses: &HashMap<String, ProjectStatus>,
    ) -> Vec<Action> {
        for session in sessions {
            let Some(pending) = &session.pending else {
                continue;
            };
            let key = question_key(&session.id, pending.request_id);
            if self.eligibility.get(&key) != Some(&Eligibility::Checking) {
                continue;
            }
            // Only a status that really says "off" rules a question out for good.
            let project = project_key(&session.cwd);
            if let Some(status) = statuses.get(&project) {
                self.delays
                    .insert(project, forward_delay_ms(Some(status.delay_secs)));
                let verdict = if status.can_forward() {
                    Eligibility::Yes
                } else {
                    Eligibility::No
                };
                self.eligibility.insert(key, verdict);
            }
        }

        // The status is how a delay changed in the settings reaches questions that are
        // already waiting; it is read before deciding, so a longer delay holds one back.
        let candidates: Vec<&SessionInfo> = sessions
            .iter()
            .filter(|session| {
                session.pending.as_ref().is_some_and(|pending| {
                    let key = question_key(&session.id, pending.request_id);
                    self.eligibility.get(&key) == Some(&Eligibility::Yes) && self.can_try(&key)
                })
            })
            .collect();
        for session in &candidates {
            let project = project_key(&session.cwd);
            if let Some(status) = statuses.get(&project) {
                self.delays
                    .insert(project, forward_delay_ms(Some(status.delay_secs)));
            }
        }
        let due: Vec<&SessionInfo> =
            due_forwards(candidates, &self.first_seen, &self.sent, now, |session| {
                self.delays
                    .get(&project_key(&session.cwd))
                    .copied()
                    .unwrap_or(FORWARD_DELAY_MS)
            });

        let mut actions = Vec::new();
        for session in due {
            let project = project_key(&session.cwd);
            let Some(status) = statuses.get(&project).filter(|s| s.can_forward()) else {
                continue;
            };
            let Some(payload) = forward_payload(session, &project, status.delay_secs) else {
                continue;
            };
            self.in_flight
                .insert(question_key(&session.id, payload.request_id));
            actions.push(Action::Forward(payload));
        }
        actions
    }

    /// The result of an `Action::Forward`. A failure costs an attempt (at most
    /// `MAX_FORWARD_ATTEMPTS`); a success makes the question one Telegram may need closed.
    pub fn forward_done(
        &mut self,
        now: u64,
        sessions: &[SessionInfo],
        session_id: &str,
        request_id: u64,
        ok: bool,
    ) -> Vec<Action> {
        let key = question_key(session_id, request_id);
        self.in_flight.remove(&key);
        if ok {
            self.sent.insert(key.clone());
            self.sent_info
                .insert(key, (session_id.to_string(), request_id));
        } else {
            *self.attempts.entry(key).or_insert(0) += 1;
        }
        self.sync(now, sessions)
    }

    /// Checks an answer from Telegram against the question that is pending now. A request
    /// id alone is not enough, so the option ids must fit the question too.
    pub fn on_answer(&mut self, event: AnswerEvent, sessions: &[SessionInfo]) -> Vec<Action> {
        let stale = |key: String| vec![Action::AnswerResult { key, ok: false }];
        let Some(pending) = sessions
            .iter()
            .find(|session| session.id == event.session_id)
            .and_then(|session| session.pending.as_ref())
            .filter(|pending| pending.request_id == event.request_id)
        else {
            return stale(event.key);
        };
        // A desktop answer records its outcome before the snapshot drops the question, so a
        // recorded outcome means the question is already resolved though it looks pending.
        if self
            .peek_outcome(&event.session_id, event.request_id)
            .is_some()
        {
            return stale(event.key);
        }
        let Some(answers) = plan(&event.reply, pending) else {
            return stale(event.key);
        };
        self.by_telegram
            .insert(question_key(&event.session_id, event.request_id));
        vec![Action::Apply(ApplyAnswer {
            result_key: event.key,
            session_id: event.session_id,
            request_id: event.request_id,
            question_id: pending.id.clone(),
            answers,
        })]
    }

    /// `on_answer` for the raw event payload. A payload that cannot be read is answered
    /// as failed when it still names its key.
    pub fn on_answer_json(&mut self, payload: &Value, sessions: &[SessionInfo]) -> Vec<Action> {
        match AnswerEvent::from_json(payload) {
            Some(event) => self.on_answer(event, sessions),
            None => payload
                .get("key")
                .and_then(Value::as_str)
                .map(|key| {
                    vec![Action::AnswerResult {
                        key: key.to_string(),
                        ok: false,
                    }]
                })
                .unwrap_or_default(),
        }
    }

    /// The result of an `Action::Apply`: returns the `Action::AnswerResult` to carry out.
    pub fn answer_applied(&mut self, apply: &ApplyAnswer, ok: bool) -> Action {
        if !ok {
            self.by_telegram
                .remove(&question_key(&apply.session_id, apply.request_id));
        }
        Action::AnswerResult {
            key: apply.result_key.clone(),
            ok,
        }
    }
}

/// What to do with a reply: `None` when it does not fit the question, `Some(None)` to skip
/// it, `Some(Some(answers))` to answer with option labels and typed text.
///
/// Beyond the ids having to exist (`replyFitsQuestion`), the native card is strict: typed
/// text needs a question that takes it, a single-choice question takes one answer, and an
/// answer has to carry something.
fn plan(reply: &Reply, pending: &Pending) -> Option<Option<Vec<Vec<String>>>> {
    let Reply::Answered { answers, custom } = reply else {
        return Some(None);
    };
    for (id, ids) in answers {
        let question = pending.questions.iter().find(|q| q.id == *id)?;
        let unknown = ids
            .iter()
            .any(|id| id != CUSTOM_OPTION_ID && !question.options.iter().any(|o| o.id == *id));
        if unknown {
            return None;
        }
    }
    if !custom
        .keys()
        .all(|id| pending.questions.iter().any(|q| q.id == *id))
    {
        return None;
    }
    let mut out = Vec::new();
    let mut carries = false;
    for question in &pending.questions {
        let ids = answers.get(&question.id).map_or(&[][..], Vec::as_slice);
        let text = custom
            .get(&question.id)
            .map(|text| text.trim())
            .filter(|text| !text.is_empty());
        let mut labels: Vec<String> = Vec::new();
        let mut typed = false;
        for id in ids {
            if id == CUSTOM_OPTION_ID {
                if !question.allow_custom {
                    return None;
                }
                if !std::mem::replace(&mut typed, true) {
                    labels.push(text?.to_string());
                }
                continue;
            }
            let option = question.options.iter().find(|o| o.id == *id)?;
            match text.filter(|_| question.allow_custom && is_other(option)) {
                Some(text) => {
                    if !std::mem::replace(&mut typed, true) {
                        labels.push(text.to_string());
                    }
                }
                None => labels.push(option.label.clone()),
            }
        }
        if !question.multi_select && labels.len() > 1 {
            return None;
        }
        carries |= !labels.is_empty();
        out.push(labels);
    }
    carries.then_some(Some(out))
}

#[cfg(test)]
mod tests;
