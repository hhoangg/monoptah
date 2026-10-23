//! Rate-limited send queue for the Telegram bridge.
//!
//! The queue is pure: time is passed in as `now_ms`, nothing sleeps, and nothing does I/O.
//! The caller asks `next_ready` for a job, sends it, and uses `next_wake` to know when to
//! ask again. Releasing a job does not spend any budget: the caller reports each HTTP call
//! with `record_send` right before it makes it, so a job that is dropped without a call
//! (a prompt closed while it waited) costs nothing.

use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// Sends allowed per chat inside `CHAT_WINDOW_MS`.
const CHAT_WINDOW_SENDS: usize = 20;
const CHAT_WINDOW_MS: u64 = 60_000;
/// Minimum gap between two sends to the same chat.
const CHAT_SPACING_MS: u64 = 1_000;
/// Sends allowed across all chats inside `GLOBAL_WINDOW_MS`.
const GLOBAL_WINDOW_SENDS: usize = 30;
const GLOBAL_WINDOW_MS: u64 = 1_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    CreateTopic {
        project: String,
        session_id: String,
        name: String,
    },
    /// Sends one question message. A prompt with several questions queues one job per
    /// question, so each message counts against the spacing and the minute budget.
    SendQuestion {
        /// Key of the pending prompt in the bridge state.
        key: String,
        /// Which question of the prompt this message carries.
        q_index: usize,
    },
    /// Rewrites a forwarded message. `keyboard` is `None` once the question is settled, so
    /// the buttons disappear; an open multi-select question keeps its (updated) keyboard.
    EditStatus {
        /// Project whose bot sent the message; it must also edit it.
        project: String,
        chat: i64,
        message_id: i64,
        html: String,
        keyboard: Option<Value>,
    },
    /// A plain message into the chat (command replies, confirmations). The chat is the
    /// queue's own chat, so it cannot go stale when a group migrates.
    Notice {
        /// Project whose bot sends the message.
        project: String,
        thread: Option<i64>,
        html: String,
    },
}

impl Job {
    /// New questions and topic creation go before status edits.
    fn is_urgent(&self) -> bool {
        !matches!(self, Job::EditStatus { .. })
    }
}

#[derive(Default)]
struct ChatQueue {
    urgent: VecDeque<Job>,
    edits: VecDeque<Job>,
    /// Release times of the most recent sends, oldest first, at most `CHAT_WINDOW_SENDS`.
    sends: VecDeque<u64>,
    paused_until: u64,
}

impl ChatQueue {
    fn is_empty(&self) -> bool {
        self.urgent.is_empty() && self.edits.is_empty()
    }

    /// Earliest time this chat's own limits (pause, spacing, minute window) allow a send.
    fn ready_at(&self) -> u64 {
        let mut at = self.paused_until;
        if let Some(last) = self.sends.back() {
            at = at.max(last + CHAT_SPACING_MS);
        }
        if self.sends.len() >= CHAT_WINDOW_SENDS {
            at = at.max(self.sends[self.sends.len() - CHAT_WINDOW_SENDS] + CHAT_WINDOW_MS);
        }
        at
    }

    fn record(&mut self, now_ms: u64) {
        self.sends.push_back(now_ms);
        while self.sends.len() > CHAT_WINDOW_SENDS {
            self.sends.pop_front();
        }
    }
}

#[derive(Default)]
pub struct SendQueue {
    chats: BTreeMap<i64, ChatQueue>,
    /// Release times of the most recent sends across all chats, oldest first.
    global_sends: VecDeque<u64>,
}

impl SendQueue {
    /// Queues a job. Queued `EditStatus` jobs for the same message are replaced by it, so only
    /// the latest text is sent. That includes one put back at the front after a failed send,
    /// otherwise the older text could go out after the newer one.
    pub fn push(&mut self, chat: i64, job: Job) {
        let queue = self.chats.entry(chat).or_default();
        if let Job::EditStatus { message_id, .. } = &job {
            let id = *message_id;
            let same = |queued: &Job| matches!(queued, Job::EditStatus { message_id, .. } if *message_id == id);
            if let Some(first) = queue.edits.iter().position(same) {
                queue.edits[first] = job;
                let mut index = 0;
                queue.edits.retain(|queued| {
                    index += 1;
                    index - 1 == first || !same(queued)
                });
                return;
            }
        }
        if job.is_urgent() {
            queue.urgent.push_back(job);
        } else {
            queue.edits.push_back(job);
        }
    }

    /// Releases the next job the budgets and pauses allow at `now_ms`. The send is not
    /// counted until the caller reports it with `record_send`.
    pub fn next_ready(&mut self, now_ms: u64) -> Option<(i64, Job)> {
        if self.global_ready_at() > now_ms {
            return None;
        }
        // Chats with an urgent job win; among equals the chat that sent least recently goes first.
        let chat = self
            .chats
            .iter()
            .filter(|(_, queue)| !queue.is_empty() && queue.ready_at() <= now_ms)
            .min_by_key(|(_, queue)| {
                (
                    queue.urgent.is_empty(),
                    queue.sends.back().copied().unwrap_or(0),
                )
            })
            .map(|(chat, _)| *chat)?;
        let queue = self.chats.get_mut(&chat)?;
        let job = queue
            .urgent
            .pop_front()
            .or_else(|| queue.edits.pop_front())?;
        Some((chat, job))
    }

    /// Counts one HTTP call to `chat` against the chat and bot budgets. Call it right before
    /// the request, once per request.
    pub fn record_send(&mut self, chat: i64, now_ms: u64) {
        self.chats.entry(chat).or_default().record(now_ms);
        self.global_sends.push_back(now_ms);
        while self.global_sends.len() > GLOBAL_WINDOW_SENDS {
            self.global_sends.pop_front();
        }
    }

    /// Earliest time at which some queued job could be released, or `None` when empty.
    pub fn next_wake(&self, now_ms: u64) -> Option<u64> {
        let chat_at = self
            .chats
            .values()
            .filter(|queue| !queue.is_empty())
            .map(ChatQueue::ready_at)
            .min()?;
        Some(chat_at.max(self.global_ready_at()).max(now_ms))
    }

    /// Blocks a chat until `until_ms` (HTTP 429 or backoff). Never shortens an earlier pause.
    pub fn pause(&mut self, chat: i64, until_ms: u64) {
        let queue = self.chats.entry(chat).or_default();
        queue.paused_until = queue.paused_until.max(until_ms);
    }

    /// Drops every queued `SendQuestion` of the prompt stored under `key`. Used when the key
    /// is about to belong to a new prompt, so old jobs cannot send the new questions out of
    /// order.
    pub fn purge_questions(&mut self, key: &str) {
        let stale = |job: &Job| matches!(job, Job::SendQuestion { key: k, .. } if k == key);
        for queue in self.chats.values_mut() {
            queue.urgent.retain(|job| !stale(job));
        }
    }

    /// Drops every queued job that belongs to `project`: its topic creations, status edits
    /// and notices, plus the question jobs of the prompts in `keys`. Returns what was dropped.
    pub fn purge_project(&mut self, project: &str, keys: &[String]) -> Vec<Job> {
        let ours = |job: &Job| match job {
            Job::CreateTopic { project: p, .. }
            | Job::EditStatus { project: p, .. }
            | Job::Notice { project: p, .. } => p == project,
            Job::SendQuestion { key, .. } => keys.contains(key),
        };
        let mut dropped = Vec::new();
        for queue in self.chats.values_mut() {
            for list in [&mut queue.urgent, &mut queue.edits] {
                let (mine, rest): (VecDeque<Job>, VecDeque<Job>) =
                    std::mem::take(list).into_iter().partition(|job| ours(job));
                *list = rest;
                dropped.extend(mine);
            }
        }
        dropped
    }

    /// Puts a job back at the front of its priority class so a retry goes first.
    pub fn requeue_front(&mut self, chat: i64, job: Job) {
        let queue = self.chats.entry(chat).or_default();
        if job.is_urgent() {
            queue.urgent.push_front(job);
        } else {
            queue.edits.push_front(job);
        }
    }

    /// Moves everything queued for `from` to `to` (a group migrated to a supergroup).
    pub fn rename_chat(&mut self, from: i64, to: i64) {
        if from == to {
            return;
        }
        let Some(mut old) = self.chats.remove(&from) else {
            return;
        };
        for job in old.urgent.iter_mut().chain(old.edits.iter_mut()) {
            if let Job::EditStatus { chat, .. } = job {
                *chat = to;
            }
        }
        let queue = self.chats.entry(to).or_default();
        queue.urgent.append(&mut old.urgent);
        queue.edits.append(&mut old.edits);
        queue.paused_until = queue.paused_until.max(old.paused_until);
        // The migrated group keeps its send history, so its budget carries over.
        let mut sends: Vec<u64> = queue
            .sends
            .iter()
            .chain(old.sends.iter())
            .copied()
            .collect();
        sends.sort_unstable();
        let keep_from = sends.len().saturating_sub(CHAT_WINDOW_SENDS);
        queue.sends = sends.split_off(keep_from).into();
    }

    /// Test aid: how many sends of `chat` are inside the minute budget's history.
    #[cfg(test)]
    pub fn recorded(&self, chat: i64) -> usize {
        self.chats.get(&chat).map_or(0, |queue| queue.sends.len())
    }

    /// Earliest time the bot-wide budget allows another send.
    fn global_ready_at(&self) -> u64 {
        if self.global_sends.len() >= GLOBAL_WINDOW_SENDS {
            self.global_sends[self.global_sends.len() - GLOBAL_WINDOW_SENDS] + GLOBAL_WINDOW_MS
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Releases the next job and counts it as sent, the way the sender does.
    fn take(q: &mut SendQueue, now_ms: u64) -> Option<(i64, Job)> {
        let (chat, job) = q.next_ready(now_ms)?;
        q.record_send(chat, now_ms);
        Some((chat, job))
    }

    fn question(key: &str) -> Job {
        Job::SendQuestion {
            key: key.into(),
            q_index: 0,
        }
    }

    fn edit(chat: i64, message_id: i64, html: &str) -> Job {
        Job::EditStatus {
            project: String::new(),
            chat,
            message_id,
            html: html.into(),
            keyboard: None,
        }
    }

    #[test]
    fn twenty_per_minute_then_waits() {
        let mut q = SendQueue::default();
        for i in 0..21 {
            q.push(1, question(&format!("k{i}")));
        }
        let mut released = 0;
        for t in (0..=19_000).step_by(1_000) {
            assert!(take(&mut q, t).is_some(), "expected a send at t={t}");
            released += 1;
        }
        assert_eq!(released, 20);
        for t in (20_000..60_000).step_by(1_000) {
            assert!(take(&mut q, t).is_none(), "unexpected send at t={t}");
        }
        assert_eq!(q.next_wake(20_000), Some(60_000));
        assert_eq!(take(&mut q, 60_000), Some((1, question("k20"))));
    }

    #[test]
    fn one_second_spacing() {
        let mut q = SendQueue::default();
        q.push(1, question("a"));
        q.push(1, question("b"));
        assert_eq!(take(&mut q, 5_000), Some((1, question("a"))));
        assert_eq!(take(&mut q, 5_999), None);
        assert_eq!(q.next_wake(5_100), Some(6_000));
        assert_eq!(take(&mut q, 6_000), Some((1, question("b"))));
    }

    #[test]
    fn releasing_a_job_spends_no_budget_until_it_is_recorded() {
        let mut q = SendQueue::default();
        q.push(1, question("a"));
        q.push(1, question("b"));
        q.push(1, question("c"));
        // A job dropped without an HTTP call leaves the budget alone.
        assert_eq!(q.next_ready(5_000), Some((1, question("a"))));
        assert_eq!(q.next_wake(5_000), Some(5_000));
        assert_eq!(q.next_ready(5_000), Some((1, question("b"))));
        // The send that is recorded does count, for the spacing and for next_wake.
        q.record_send(1, 5_000);
        assert_eq!(q.next_ready(5_999), None);
        assert_eq!(q.next_wake(5_100), Some(6_000));
        assert_eq!(q.next_ready(6_000), Some((1, question("c"))));
    }

    #[test]
    fn questions_before_edits() {
        let mut q = SendQueue::default();
        q.push(1, edit(1, 10, "edit"));
        q.push(1, question("q1"));
        q.push(
            1,
            Job::CreateTopic {
                project: "p".into(),
                session_id: "s".into(),
                name: "n".into(),
            },
        );
        assert_eq!(take(&mut q, 0), Some((1, question("q1"))));
        assert!(matches!(
            take(&mut q, 1_000),
            Some((1, Job::CreateTopic { .. }))
        ));
        assert_eq!(take(&mut q, 2_000), Some((1, edit(1, 10, "edit"))));
        assert_eq!(take(&mut q, 3_000), None);
    }

    #[test]
    fn edits_to_same_message_coalesce() {
        let mut q = SendQueue::default();
        q.push(1, edit(1, 10, "first"));
        q.push(1, edit(1, 11, "other"));
        q.push(1, edit(1, 10, "second"));
        // Same message id in another chat is a different message.
        q.push(2, edit(2, 10, "elsewhere"));
        assert_eq!(take(&mut q, 0), Some((1, edit(1, 10, "second"))));
        assert_eq!(take(&mut q, 1_000), Some((1, edit(1, 11, "other"))));
        assert_eq!(take(&mut q, 1_000), Some((2, edit(2, 10, "elsewhere"))));
        assert_eq!(take(&mut q, 5_000), None);
    }

    #[test]
    fn push_removes_a_requeued_edit_for_the_same_message() {
        let mut q = SendQueue::default();
        q.push(1, edit(1, 10, "e1"));
        // E1 goes out, and while it is in flight E2 for the same message is queued.
        let (_, e1) = take(&mut q, 0).unwrap();
        q.push(1, edit(1, 10, "e2"));
        // E1 failed on the network and goes back to the front, ahead of E2.
        q.requeue_front(1, e1);
        // E3 supersedes both; neither old text may be sent after it.
        q.push(1, edit(1, 10, "e3"));
        q.push(1, edit(1, 11, "other"));
        assert_eq!(take(&mut q, 10_000), Some((1, edit(1, 10, "e3"))));
        assert_eq!(take(&mut q, 11_000), Some((1, edit(1, 11, "other"))));
        assert_eq!(take(&mut q, 12_000), None);
    }

    #[test]
    fn pause_blocks_until() {
        let mut q = SendQueue::default();
        q.push(1, question("a"));
        q.push(2, question("b"));
        q.pause(1, 10_000);
        // The other chat is not affected.
        assert_eq!(take(&mut q, 0), Some((2, question("b"))));
        assert_eq!(take(&mut q, 9_999), None);
        assert_eq!(q.next_wake(0), Some(10_000));
        assert_eq!(take(&mut q, 10_000), Some((1, question("a"))));
        // A shorter pause never shortens a longer one.
        q.push(1, question("c"));
        q.pause(1, 20_000);
        q.pause(1, 15_000);
        assert_eq!(take(&mut q, 19_999), None);
        assert_eq!(take(&mut q, 20_000), Some((1, question("c"))));
    }

    #[test]
    fn requeue_front_goes_first() {
        let mut q = SendQueue::default();
        q.push(1, question("a"));
        q.push(1, question("b"));
        q.push(1, edit(1, 10, "e1"));
        q.push(1, edit(1, 11, "e2"));
        let (_, first) = take(&mut q, 0).unwrap();
        assert_eq!(first, question("a"));
        q.requeue_front(1, first);
        assert_eq!(take(&mut q, 1_000), Some((1, question("a"))));
        assert_eq!(take(&mut q, 2_000), Some((1, question("b"))));
        let (_, e1) = take(&mut q, 3_000).unwrap();
        assert_eq!(e1, edit(1, 10, "e1"));
        q.requeue_front(1, e1);
        assert_eq!(take(&mut q, 4_000), Some((1, edit(1, 10, "e1"))));
        assert_eq!(take(&mut q, 5_000), Some((1, edit(1, 11, "e2"))));
    }

    #[test]
    fn rename_chat_moves_jobs() {
        let mut q = SendQueue::default();
        q.push(1, question("a"));
        q.push(1, edit(1, 10, "e"));
        q.pause(1, 5_000);
        q.rename_chat(1, 2);
        assert_eq!(take(&mut q, 4_999), None);
        assert_eq!(take(&mut q, 5_000), Some((2, question("a"))));
        // The edit now addresses the new chat.
        assert_eq!(take(&mut q, 6_000), Some((2, edit(2, 10, "e"))));
        assert_eq!(q.next_wake(7_000), None);
    }

    #[test]
    fn global_thirty_per_second() {
        let mut q = SendQueue::default();
        for chat in 0..31 {
            q.push(chat, question(&format!("k{chat}")));
        }
        for _ in 0..30 {
            assert!(take(&mut q, 0).is_some());
        }
        assert_eq!(take(&mut q, 0), None);
        assert_eq!(take(&mut q, 999), None);
        assert_eq!(q.next_wake(0), Some(1_000));
        assert!(take(&mut q, 1_000).is_some());
        assert_eq!(q.next_wake(1_000), None);
    }
}
