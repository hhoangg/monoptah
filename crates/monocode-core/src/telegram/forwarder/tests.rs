use super::*;
use crate::telegram::forward::{SessionInfo, FORWARD_DELAY_MS};
use crate::telegram::render::{ForwardPayload, OptionIn, QuestionIn, CUSTOM_OPTION_ID};
use serde_json::json;
use std::collections::BTreeMap;

const START: u64 = 1_000_000;

fn enabled() -> ProjectStatus {
    ProjectStatus {
        configured: true,
        enabled: true,
        has_chat: true,
        delay_secs: 300,
    }
}

fn option(id: &str, label: &str) -> OptionIn {
    OptionIn {
        id: id.into(),
        label: label.into(),
        description: None,
    }
}

fn question(id: &str, multi: bool, custom: bool, options: Vec<OptionIn>) -> QuestionIn {
    QuestionIn {
        id: id.into(),
        header: None,
        prompt: format!("Prompt {id}?"),
        multi_select: multi,
        allow_custom: custom,
        options,
    }
}

fn asking_with(request_id: u64, id: &str, auto: bool, questions: Vec<QuestionIn>) -> SessionInfo {
    SessionInfo {
        id: id.into(),
        cwd: "/repo".into(),
        title: "Task".into(),
        provider: "codex".into(),
        pending: Some(Pending {
            id: format!("wire-{request_id}"),
            request_id,
            questions,
            auto_resolve: auto,
        }),
        ..Default::default()
    }
}

fn asking(request_id: u64, id: &str) -> SessionInfo {
    asking_with(
        request_id,
        id,
        false,
        vec![question("q1", false, false, vec![option("a", "A")])],
    )
}

fn idle(mut session: SessionInfo) -> SessionInfo {
    session.pending = None;
    session
}

fn answered(pairs: &[(&str, &[&str])], custom: &[(&str, &str)]) -> Reply {
    Reply::Answered {
        answers: pairs
            .iter()
            .map(|(q, ids)| (q.to_string(), ids.iter().map(|i| i.to_string()).collect()))
            .collect(),
        custom: custom
            .iter()
            .map(|(q, t)| (q.to_string(), t.to_string()))
            .collect(),
    }
}

fn event(request_id: u64, reply: Reply) -> AnswerEvent {
    AnswerEvent {
        key: "pending-1".into(),
        session_id: "s1".into(),
        request_id,
        reply,
        by: "@someone".into(),
    }
}

/// Plays the driver: a 15 s tick, a status cache, and immediate results for every action.
struct Harness {
    forwarder: Forwarder,
    sessions: Vec<SessionInfo>,
    cache: StatusCache,
    /// What a status read returns; `None` is a failed read.
    status: Option<ProjectStatus>,
    status_reads: usize,
    clock: u64,
    next_tick: u64,
    forward_ok: bool,
    apply_ok: bool,
    forwards: Vec<ForwardPayload>,
    closes: Vec<(String, u64, &'static str)>,
    results: Vec<(String, bool)>,
    applies: Vec<ApplyAnswer>,
}

impl Harness {
    fn new() -> Self {
        Self {
            forwarder: Forwarder::default(),
            sessions: Vec::new(),
            cache: StatusCache::default(),
            status: Some(enabled()),
            status_reads: 0,
            clock: START,
            next_tick: START + TICK_MS,
            forward_ok: true,
            apply_ok: true,
            forwards: Vec::new(),
            closes: Vec::new(),
            results: Vec::new(),
            applies: Vec::new(),
        }
    }

    fn statuses(&mut self) -> HashMap<String, ProjectStatus> {
        let mut statuses = HashMap::new();
        for project in self.forwarder.projects_to_read(&self.sessions) {
            if let Some(status) = self.cache.get(&project, self.clock) {
                statuses.insert(project, status.clone());
                continue;
            }
            self.status_reads += 1;
            if let Some(status) = self.status.clone() {
                self.cache.put(&project, self.clock, status.clone());
                statuses.insert(project, status);
            }
        }
        statuses
    }

    fn handle(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Forward(payload) => {
                    self.forwards.push(payload.clone());
                    let more = self.forwarder.forward_done(
                        self.clock,
                        &self.sessions,
                        &payload.session_id,
                        payload.request_id,
                        self.forward_ok,
                    );
                    self.handle(more);
                }
                Action::Close {
                    session_id,
                    request_id,
                    status,
                } => self.closes.push((session_id, request_id, status)),
                Action::Apply(apply) => {
                    let result = self.forwarder.answer_applied(&apply, self.apply_ok);
                    self.applies.push(apply);
                    self.handle(vec![result]);
                }
                Action::AnswerResult { key, ok } => self.results.push((key, ok)),
            }
        }
    }

    fn run(&mut self) {
        let actions = self.forwarder.sync(self.clock, &self.sessions);
        self.handle(actions);
        let statuses = self.statuses();
        let actions = self
            .forwarder
            .forward(self.clock, &self.sessions, &statuses);
        self.handle(actions);
    }

    fn render(&mut self, sessions: Vec<SessionInfo>) {
        self.sessions = sessions;
        self.run();
    }

    fn advance(&mut self, ms: u64) {
        let target = self.clock + ms;
        while self.next_tick <= target {
            self.clock = self.next_tick;
            self.next_tick += TICK_MS;
            self.run();
        }
        self.clock = target;
    }

    fn deliver(&mut self, event: AnswerEvent) {
        let actions = self.forwarder.on_answer(event, &self.sessions);
        self.handle(actions);
    }

    fn invalidate(&mut self) {
        self.cache.invalidate(None);
    }

    /// A session whose question was forwarded.
    fn forwarded(session: SessionInfo) -> Self {
        let mut harness = Self::new();
        harness.render(vec![session]);
        harness.advance(FORWARD_DELAY_MS + TICK_MS);
        assert_eq!(harness.forwards.len(), 1);
        harness
    }
}

#[test]
fn forwards_after_five_minutes_when_enabled() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.advance(FORWARD_DELAY_MS - TICK_MS);
    assert_eq!(h.forwards.len(), 0);
    h.advance(TICK_MS * 2);
    assert_eq!(h.forwards.len(), 1);
    assert_eq!(h.forwards[0].session_id, "s1");
    assert_eq!(h.forwards[0].request_id, 1);
    assert_eq!(h.forwards[0].project, "/repo");
    h.advance(TICK_MS * 4);
    assert_eq!(h.forwards.len(), 1);
}

#[test]
fn sends_the_configured_delay_in_the_payload() {
    let mut h = Harness::new();
    h.status = Some(ProjectStatus {
        delay_secs: 120,
        ..enabled()
    });
    h.render(vec![asking(1, "s1")]);
    h.advance(120_000 + TICK_MS);
    assert_eq!(h.forwards[0].delay_secs, 120);
}

#[test]
fn forwards_right_away_when_the_delay_is_zero() {
    let mut h = Harness::new();
    h.status = Some(ProjectStatus {
        delay_secs: 0,
        ..enabled()
    });
    h.render(vec![asking(1, "s1")]);
    assert_eq!(h.forwards.len(), 1);
    h.advance(TICK_MS * 4);
    assert_eq!(h.forwards.len(), 1);
    assert_eq!(h.forwards[0].delay_secs, 0);
    assert_eq!(
        (h.forwards[0].session_id.as_str(), h.forwards[0].request_id),
        ("s1", 1)
    );
}

#[test]
fn never_forwards_a_question_first_seen_while_disabled_even_with_no_delay() {
    let mut h = Harness::new();
    h.status = Some(ProjectStatus {
        enabled: false,
        delay_secs: 0,
        ..enabled()
    });
    h.render(vec![asking(1, "s1")]);
    h.advance(TICK_MS);
    h.status = Some(ProjectStatus {
        delay_secs: 0,
        ..enabled()
    });
    h.invalidate();
    h.advance(TICK_MS * 4);
    assert_eq!(h.forwards.len(), 0);
}

#[test]
fn waits_for_a_short_delay_and_not_a_tick_longer() {
    let mut h = Harness::new();
    h.status = Some(ProjectStatus {
        delay_secs: 60,
        ..enabled()
    });
    h.render(vec![asking(1, "s1")]);
    h.advance(30_000);
    assert_eq!(h.forwards.len(), 0);
    h.advance(30_000);
    assert_eq!(h.forwards.len(), 1);
}

#[test]
fn applies_a_delay_changed_while_the_question_waits() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.advance(60_000);
    assert_eq!(h.forwards.len(), 0);
    // The settings dialog saves the new value and drops the cached status.
    h.status = Some(ProjectStatus {
        delay_secs: 0,
        ..enabled()
    });
    h.invalidate();
    h.advance(TICK_MS);
    assert_eq!(h.forwards.len(), 1);
}

#[test]
fn holds_a_question_back_when_the_delay_is_made_longer() {
    let mut h = Harness::new();
    h.status = Some(ProjectStatus {
        delay_secs: 60,
        ..enabled()
    });
    h.render(vec![asking(1, "s1")]);
    h.advance(30_000);
    h.status = Some(ProjectStatus {
        delay_secs: 600,
        ..enabled()
    });
    h.invalidate();
    h.advance(TICK_MS * 3);
    assert_eq!(h.forwards.len(), 0);
    h.advance(600_000);
    assert_eq!(h.forwards.len(), 1);
}

#[test]
fn does_not_forward_when_answered_within_five_minutes() {
    let session = asking(1, "s1");
    let mut h = Harness::new();
    h.render(vec![session.clone()]);
    h.advance(FORWARD_DELAY_MS - 60_000);
    h.render(vec![idle(session)]);
    h.advance(FORWARD_DELAY_MS);
    assert_eq!(h.forwards.len(), 0);
    assert_eq!(h.closes.len(), 0);
}

#[test]
fn never_forwards_a_question_first_seen_while_disabled() {
    let mut h = Harness::new();
    h.status = Some(ProjectStatus {
        enabled: false,
        ..enabled()
    });
    h.render(vec![asking(1, "s1")]);
    h.advance(FORWARD_DELAY_MS + TICK_MS);
    h.status = Some(enabled());
    h.advance(120_000);
    assert_eq!(h.forwards.len(), 0);
}

#[test]
fn a_project_without_a_chat_or_token_is_not_forwarded() {
    for status in [
        ProjectStatus {
            has_chat: false,
            ..enabled()
        },
        ProjectStatus {
            configured: false,
            ..enabled()
        },
    ] {
        let mut h = Harness::new();
        h.status = Some(status);
        h.render(vec![asking(1, "s1")]);
        h.advance(FORWARD_DELAY_MS + TICK_MS);
        assert_eq!(h.forwards.len(), 0);
    }
}

#[test]
fn retries_an_unavailable_status_on_the_next_tick_and_then_forwards() {
    let mut h = Harness::new();
    h.status = None;
    h.render(vec![asking(1, "s1")]);
    h.advance(TICK_MS * 3);
    assert!(h.status_reads > 1);
    h.status = Some(enabled());
    h.advance(TICK_MS);
    h.advance(FORWARD_DELAY_MS);
    assert_eq!(h.forwards.len(), 1);
}

#[test]
fn does_not_cache_a_failed_status() {
    let mut h = Harness::new();
    h.status = None;
    h.render(vec![asking(1, "s1")]);
    let failed = h.status_reads;
    h.status = Some(enabled());
    h.render(vec![asking(1, "s1"), asking(1, "s2")]);
    assert!(h.status_reads > failed);
}

#[test]
fn keeps_a_successful_disabled_status_final_even_if_retries_came_first() {
    let mut h = Harness::new();
    h.status = None;
    h.render(vec![asking(1, "s1")]);
    h.advance(TICK_MS);
    h.status = Some(ProjectStatus {
        enabled: false,
        ..enabled()
    });
    h.advance(TICK_MS);
    h.status = Some(enabled());
    h.invalidate();
    h.advance(FORWARD_DELAY_MS + TICK_MS * 6);
    assert_eq!(h.forwards.len(), 0);
}

#[test]
fn asks_for_a_fresh_status_after_the_cache_is_invalidated() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    assert_eq!(h.status_reads, 1);
    h.render(vec![asking(1, "s1"), asking(1, "s2")]);
    assert_eq!(h.status_reads, 1);
    h.invalidate();
    h.render(vec![asking(1, "s1"), asking(1, "s2"), asking(1, "s3")]);
    assert_eq!(h.status_reads, 2);
}

#[test]
fn never_forwards_a_non_blocking_question_even_after_its_timer_is_cleared() {
    let mut h = Harness::new();
    let mut timed = asking(1, "s1");
    timed.pending.as_mut().unwrap().auto_resolve = true;
    h.render(vec![timed.clone()]);
    h.advance(TICK_MS);
    timed.pending.as_mut().unwrap().auto_resolve = false;
    h.render(vec![timed]);
    h.advance(FORWARD_DELAY_MS + TICK_MS);
    assert_eq!(h.forwards.len(), 0);
}

#[test]
fn excluded_sessions_are_never_forwarded_and_never_read_a_status() {
    let mut mono = asking(1, "s1");
    mono.mono = true;
    let mut worker = asking(1, "s2");
    worker.orchestration_worker = true;
    let mut tui = asking(1, "s3");
    tui.surface = crate::surface::Surface::Tui;
    let mut remote = asking(1, "s4");
    remote.cwd = "remote://host/repo".into();
    let mut h = Harness::new();
    h.render(vec![mono, worker, tui, remote]);
    h.advance(FORWARD_DELAY_MS + TICK_MS);
    assert_eq!(h.forwards.len(), 0);
    assert_eq!(h.status_reads, 0);
}

#[test]
fn statuses_are_cached_between_ticks_and_expire_after_a_minute() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.advance(TICK_MS * 3);
    assert_eq!(h.status_reads, 1);
    h.advance(STATUS_TTL_MS);
    assert_eq!(h.status_reads, 2);
}

#[test]
fn the_status_cache_keeps_projects_apart() {
    let mut cache = StatusCache::default();
    cache.put("/a", 10, enabled());
    assert!(cache.get("/a", 10 + STATUS_TTL_MS - 1).is_some());
    assert!(cache.get("/a", 10 + STATUS_TTL_MS).is_none());
    assert!(cache.get("/b", 10).is_none());
    cache.put("/b", 10, enabled());
    cache.invalidate(Some("/a"));
    assert!(cache.get("/a", 11).is_none());
    assert!(cache.get("/b", 11).is_some());
    cache.invalidate(None);
    assert!(cache.get("/b", 11).is_none());
}

#[test]
fn a_telegram_status_becomes_a_project_status() {
    let status = crate::telegram::TelegramStatus {
        configured: true,
        enabled: true,
        chat_id: Some(-100),
        delay_secs: 45,
        ..Default::default()
    };
    let project = ProjectStatus::from(&status);
    assert_eq!(
        project,
        ProjectStatus {
            delay_secs: 45,
            ..enabled()
        }
    );
    assert!(project.can_forward());
    let off = ProjectStatus::from(&crate::telegram::TelegramStatus::default());
    assert!(!off.can_forward());
}

#[test]
fn a_status_that_cannot_forward_keeps_a_waiting_question_unsent() {
    // Eligible at first sight, but the chat is gone when it comes due.
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.advance(FORWARD_DELAY_MS - TICK_MS * 2);
    h.status = Some(ProjectStatus {
        has_chat: false,
        ..enabled()
    });
    h.invalidate();
    h.advance(TICK_MS * 6);
    assert_eq!(h.forwards.len(), 0);
    // Reconnecting lets it go; a status that cannot be read costs no attempt.
    h.status = None;
    h.invalidate();
    h.advance(TICK_MS * 6);
    h.status = Some(enabled());
    h.invalidate();
    h.advance(TICK_MS);
    assert_eq!(h.forwards.len(), 1);
}

#[test]
fn routes_an_answer_to_apply_with_the_questions_native_id() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.deliver(event(1, answered(&[("q1", &["a"])], &[])));
    assert_eq!(h.applies.len(), 1);
    assert_eq!(h.applies[0].session_id, "s1");
    assert_eq!(h.applies[0].request_id, 1);
    assert_eq!(h.applies[0].question_id, "wire-1");
    assert_eq!(h.applies[0].answers, Some(vec![vec!["A".to_string()]]));
    assert_eq!(h.results, [("pending-1".to_string(), true)]);
}

#[test]
fn rejects_an_answer_whose_question_ids_do_not_match_the_current_question() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.deliver(event(1, answered(&[("old", &["a"])], &[])));
    h.deliver(event(1, answered(&[("q1", &["a"])], &[("old", "text")])));
    assert!(h.applies.is_empty());
    assert_eq!(
        h.results,
        [
            ("pending-1".to_string(), false),
            ("pending-1".to_string(), false)
        ]
    );
}

#[test]
fn rejects_an_answer_whose_option_ids_do_not_match_the_current_question() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.deliver(event(1, answered(&[("q1", &["gone"])], &[])));
    assert!(h.applies.is_empty());
    assert_eq!(h.results, [("pending-1".to_string(), false)]);
}

#[test]
fn accepts_a_skipped_reply_and_a_custom_answer() {
    let session = asking_with(
        1,
        "s1",
        false,
        vec![question("q1", false, true, vec![option("a", "A")])],
    );
    let mut h = Harness::new();
    h.render(vec![session]);
    h.deliver(event(1, Reply::Skipped));
    h.deliver(event(
        1,
        answered(&[("q1", &[CUSTOM_OPTION_ID])], &[("q1", "my text")]),
    ));
    assert_eq!(h.applies.len(), 2);
    assert_eq!(h.applies[0].answers, None);
    assert_eq!(
        h.applies[1].answers,
        Some(vec![vec!["my text".to_string()]])
    );
    assert_eq!(
        h.results,
        [
            ("pending-1".to_string(), true),
            ("pending-1".to_string(), true)
        ]
    );
}

#[test]
fn option_ids_become_labels_in_question_order_and_unanswered_questions_stay_empty() {
    let session = asking_with(
        1,
        "s1",
        false,
        vec![
            question(
                "one",
                false,
                false,
                vec![option("x", "First"), option("y", "Second")],
            ),
            question(
                "many",
                true,
                true,
                vec![option("p", "Pee"), option("q", "Cue")],
            ),
            question("last", false, false, vec![option("z", "Zed")]),
        ],
    );
    let mut h = Harness::new();
    h.render(vec![session]);
    h.deliver(event(
        1,
        answered(
            &[("many", &["q", CUSTOM_OPTION_ID, "p"]), ("one", &["y"])],
            &[("many", "  extra  ")],
        ),
    ));
    assert_eq!(
        h.applies[0].answers,
        Some(vec![
            vec!["Second".to_string()],
            vec!["Cue".to_string(), "extra".to_string(), "Pee".to_string()],
            vec![],
        ])
    );
}

#[test]
fn an_other_option_with_typed_text_answers_with_the_text() {
    let session = asking_with(
        1,
        "s1",
        false,
        vec![question(
            "q1",
            false,
            true,
            vec![option("o", "Other"), option("a", "A")],
        )],
    );
    let mut h = Harness::new();
    h.render(vec![session]);
    h.deliver(event(
        1,
        answered(&[("q1", &["o"])], &[("q1", "something")]),
    ));
    assert_eq!(
        h.applies[0].answers,
        Some(vec![vec!["something".to_string()]])
    );
    h.deliver(event(1, answered(&[("q1", &["o"])], &[])));
    assert_eq!(h.applies[1].answers, Some(vec![vec!["Other".to_string()]]));
}

#[test]
fn rejects_custom_text_the_question_cannot_take_and_answers_that_do_not_fit() {
    let session = asking_with(
        1,
        "s1",
        false,
        vec![
            question(
                "closed",
                false,
                false,
                vec![option("a", "A"), option("b", "B")],
            ),
            question("open", false, true, vec![option("a", "A")]),
        ],
    );
    let mut h = Harness::new();
    h.render(vec![session]);
    // Custom text where the question does not allow it.
    h.deliver(event(
        1,
        answered(&[("closed", &[CUSTOM_OPTION_ID])], &[("closed", "x")]),
    ));
    // The custom option without any text.
    h.deliver(event(1, answered(&[("open", &[CUSTOM_OPTION_ID])], &[])));
    h.deliver(event(
        1,
        answered(&[("open", &[CUSTOM_OPTION_ID])], &[("open", "   ")]),
    ));
    // Two choices for a single-select question.
    h.deliver(event(1, answered(&[("closed", &["a", "b"])], &[])));
    // An answer that carries nothing.
    h.deliver(event(1, answered(&[("closed", &[])], &[])));
    h.deliver(event(1, answered(&[], &[])));
    assert!(h.applies.is_empty());
    assert_eq!(h.results.len(), 6);
    assert!(h.results.iter().all(|(_, ok)| !ok));
}

#[test]
fn ignores_an_answer_for_a_different_request_id() {
    let mut h = Harness::new();
    h.render(vec![asking(2, "s1")]);
    h.deliver(event(1, answered(&[("q1", &["a"])], &[])));
    assert!(h.applies.is_empty());
    assert_eq!(h.results, [("pending-1".to_string(), false)]);
}

#[test]
fn reports_stale_when_no_longer_pending() {
    let mut h = Harness::new();
    h.render(vec![idle(asking(1, "s1"))]);
    h.deliver(event(1, answered(&[("q1", &["a"])], &[])));
    assert!(h.applies.is_empty());
    assert_eq!(h.results, [("pending-1".to_string(), false)]);
}

#[test]
fn reports_stale_when_the_desktop_already_resolved_a_question_that_still_looks_pending() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.forwarder.record_outcome("s1", 1, Outcome::Answered);
    h.deliver(event(1, answered(&[("q1", &["a"])], &[])));
    assert!(h.applies.is_empty());
    assert_eq!(h.results, [("pending-1".to_string(), false)]);
}

#[test]
fn reports_stale_for_an_unknown_session() {
    let mut h = Harness::new();
    h.render(vec![]);
    h.deliver(event(1, answered(&[("q1", &["a"])], &[])));
    assert_eq!(h.results, [("pending-1".to_string(), false)]);
}

#[test]
fn answers_against_the_snapshot_it_is_given() {
    let session = asking(1, "s1");
    let mut h = Harness::new();
    h.render(vec![session.clone()]);
    h.render(vec![idle(session)]);
    h.deliver(event(1, answered(&[("q1", &["a"])], &[])));
    assert!(h.applies.is_empty());
}

#[test]
fn reports_failure_once_when_applying_the_answer_fails_and_still_closes_the_question() {
    let session = asking(1, "s1");
    let mut h = Harness::forwarded(session.clone());
    h.apply_ok = false;
    h.deliver(event(1, answered(&[("q1", &["a"])], &[])));
    assert_eq!(h.results, [("pending-1".to_string(), false)]);
    // Telegram did not answer it after all, so the desktop outcome is reported.
    h.forwarder.record_outcome("s1", 1, Outcome::Answered);
    h.render(vec![idle(session)]);
    assert_eq!(h.closes, [("s1".to_string(), 1, "Answered on desktop")]);
}

#[test]
fn answer_events_parse_from_the_sinks_json() {
    let parsed = AnswerEvent::from_json(&json!({
        "key": "pending-9",
        "sessionId": "s1",
        "requestId": 4,
        "reply": {"kind": "answered", "answers": {"q1": ["a", "b"]}, "custom": {"q1": "t"}},
        "by": "@me",
    }))
    .unwrap();
    assert_eq!(parsed.key, "pending-9");
    assert_eq!(parsed.session_id, "s1");
    assert_eq!(parsed.request_id, 4);
    assert_eq!(parsed.by, "@me");
    assert_eq!(
        parsed.reply,
        Reply::Answered {
            answers: BTreeMap::from([("q1".to_string(), vec!["a".to_string(), "b".to_string()])]),
            custom: BTreeMap::from([("q1".to_string(), "t".to_string())]),
        }
    );
    let skipped = AnswerEvent::from_json(&json!({
        "key": "k", "sessionId": "s", "requestId": 1, "reply": {"kind": "skipped"}, "by": "x"
    }))
    .unwrap();
    assert_eq!(skipped.reply, Reply::Skipped);
    assert!(AnswerEvent::from_json(&json!({"key": "k"})).is_none());
    assert!(AnswerEvent::from_json(&json!({
        "key": "k", "sessionId": "s", "requestId": 1, "reply": {"kind": "nope"}, "by": "x"
    }))
    .is_none());
}

#[test]
fn a_malformed_answer_payload_with_a_key_is_answered_as_failed() {
    let mut forwarder = Forwarder::default();
    let actions = forwarder.on_answer_json(&json!({"key": "k9", "reply": 3}), &[]);
    assert!(matches!(
        actions.as_slice(),
        [Action::AnswerResult { key, ok: false }] if key == "k9"
    ));
    assert!(forwarder
        .on_answer_json(&json!({"reply": 3}), &[])
        .is_empty());
}

#[test]
fn reports_a_desktop_answer_as_closed() {
    let session = asking(1, "s1");
    let mut h = Harness::forwarded(session.clone());
    h.forwarder.record_outcome("s1", 1, Outcome::Answered);
    h.render(vec![idle(session)]);
    assert_eq!(h.closes, [("s1".to_string(), 1, "Answered on desktop")]);
    h.advance(TICK_MS * 3);
    assert_eq!(h.closes.len(), 1);
}

#[test]
fn reports_a_skipped_question_as_closed() {
    let session = asking(1, "s1");
    let mut h = Harness::forwarded(session.clone());
    h.forwarder.record_outcome("s1", 1, Outcome::Skipped);
    h.render(vec![idle(session)]);
    assert_eq!(h.closes, [("s1".to_string(), 1, "Skipped")]);
}

#[test]
fn reports_a_cancelled_turn_as_closed() {
    let session = asking(1, "s1");
    let mut h = Harness::forwarded(session.clone());
    h.forwarder.record_outcome("s1", 1, Outcome::Cancelled);
    h.render(vec![idle(session)]);
    assert_eq!(h.closes, [("s1".to_string(), 1, "Cancelled")]);
}

#[test]
fn ignores_a_stale_outcome_left_by_an_earlier_question_with_the_same_key() {
    let mut h = Harness::new();
    h.forwarder.record_outcome("s1", 1, Outcome::Answered);
    h.render(vec![asking(1, "s1")]);
    h.advance(FORWARD_DELAY_MS + TICK_MS);
    assert_eq!(h.forwards.len(), 1);
    h.render(vec![]);
    assert_eq!(h.closes, [("s1".to_string(), 1, "Cancelled")]);
}

#[test]
fn reports_a_vanished_session_as_cancelled() {
    let mut h = Harness::forwarded(asking(1, "s1"));
    h.render(vec![]);
    assert_eq!(h.closes, [("s1".to_string(), 1, "Cancelled")]);
}

#[test]
fn reports_a_replaced_request_id_as_closed() {
    let mut h = Harness::forwarded(asking(1, "s1"));
    h.render(vec![asking(2, "s1")]);
    assert_eq!(h.closes, [("s1".to_string(), 1, "Cancelled")]);
}

#[test]
fn does_not_report_closure_for_its_own_delivery() {
    let session = asking(1, "s1");
    let mut h = Harness::forwarded(session.clone());
    h.deliver(event(1, answered(&[("q1", &["a"])], &[])));
    h.forwarder.record_outcome("s1", 1, Outcome::Answered);
    h.render(vec![idle(session)]);
    h.advance(TICK_MS * 2);
    assert_eq!(h.closes.len(), 0);
    assert_eq!(h.results, [("pending-1".to_string(), true)]);
}

#[test]
fn retries_a_failed_forward_once_on_the_next_tick_then_stops() {
    let mut h = Harness::new();
    h.forward_ok = false;
    h.render(vec![asking(1, "s1")]);
    h.advance(FORWARD_DELAY_MS + TICK_MS);
    h.advance(TICK_MS * 6);
    assert_eq!(h.forwards.len(), 2);
    h.forward_ok = true;
    h.advance(TICK_MS * 6);
    assert_eq!(h.forwards.len(), 2);
}

#[test]
fn recovers_after_a_failed_forward_when_the_retry_succeeds() {
    let mut h = Harness::new();
    h.forward_ok = false;
    h.render(vec![asking(1, "s1")]);
    for _ in 0..FORWARD_DELAY_MS / TICK_MS {
        h.advance(TICK_MS);
    }
    assert_eq!(h.forwards.len(), 1);
    h.forward_ok = true;
    h.advance(TICK_MS);
    assert_eq!(h.forwards.len(), 2);
    h.advance(TICK_MS * 3);
    assert_eq!(h.forwards.len(), 2);
    h.render(vec![]);
    assert_eq!(h.closes.len(), 1);
}

#[test]
fn a_question_that_closed_while_its_forward_was_in_flight_is_still_closed_on_telegram() {
    let mut h = Harness::new();
    h.render(vec![asking(1, "s1")]);
    h.advance(FORWARD_DELAY_MS - TICK_MS);
    let statuses = h.statuses();
    let actions = h
        .forwarder
        .forward(h.clock + TICK_MS, &h.sessions, &statuses);
    let Some(Action::Forward(payload)) = actions.into_iter().next() else {
        panic!("expected a forward");
    };
    // The desktop answered while the message was on its way.
    h.sessions = vec![];
    let closes = h.forwarder.sync(h.clock + TICK_MS, &h.sessions);
    assert!(closes.is_empty(), "nothing is open on Telegram yet");
    let closes = h.forwarder.forward_done(
        h.clock + TICK_MS,
        &h.sessions,
        &payload.session_id,
        payload.request_id,
        true,
    );
    assert!(matches!(
        closes.as_slice(),
        [Action::Close {
            status: "Cancelled",
            request_id: 1,
            ..
        }]
    ));
}

#[test]
fn outcomes_are_remembered_for_a_bounded_number_of_questions() {
    let mut forwarder = Forwarder::default();
    for request in 0..(MAX_OUTCOMES as u64 + 10) {
        forwarder.record_outcome("s", request, Outcome::Skipped);
    }
    assert_eq!(forwarder.peek_outcome("s", 0), None);
    assert_eq!(forwarder.peek_outcome("s", 9), None);
    assert_eq!(forwarder.peek_outcome("s", 10), Some(Outcome::Skipped));
    assert_eq!(
        forwarder.peek_outcome("s", MAX_OUTCOMES as u64 + 9),
        Some(Outcome::Skipped)
    );
    // Recording again keeps the newest decision only.
    forwarder.record_outcome("s", 10, Outcome::Cancelled);
    assert_eq!(forwarder.peek_outcome("s", 10), Some(Outcome::Cancelled));
}

#[test]
fn projects_to_read_lists_only_projects_that_need_a_status() {
    let mut other = asking(1, "s2");
    other.cwd = "/other".into();
    let mut h = Harness::new();
    h.sessions = vec![asking(1, "s1"), other, idle(asking(1, "s3"))];
    h.forwarder.sync(h.clock, &h.sessions);
    assert_eq!(
        h.forwarder.projects_to_read(&h.sessions),
        ["/other", "/repo"]
    );
    // Once forwarded, a question needs no status.
    h.run();
    h.advance(FORWARD_DELAY_MS + TICK_MS);
    assert_eq!(h.forwards.len(), 2);
    assert!(h.forwarder.projects_to_read(&h.sessions).is_empty());
}
