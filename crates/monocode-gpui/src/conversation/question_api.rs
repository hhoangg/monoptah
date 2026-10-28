//! The question API of a conversation, shared by the question card and Telegram: read the
//! question being asked, answer it, and learn how each question ended.
use super::*;
use gpui::App;
use monocode_core::{agent::Question, telegram::forwarder::Outcome};

/// A question joined the queue behind the ones already waiting.
pub(crate) struct QuestionAsked;
impl EventEmitter<QuestionAsked> for Conversation {}

/// A question left the conversation: the user answered or skipped it, or it was cancelled
/// (the turn ended or was stopped, or the provider withdrew it). `id` is `Question::id`.
pub(crate) struct QuestionSettled {
    pub id: String,
    pub outcome: Outcome,
}
impl EventEmitter<QuestionSettled> for Conversation {}

impl Conversation {
    /// The question being asked: the front of the queue, whose card covers the composer.
    pub(crate) fn pending_question(&self, cx: &App) -> Option<Question> {
        self.questions
            .front()
            .map(|(card, _)| card.read(cx).request().clone())
    }

    /// The latest assistant message, for the summary Telegram shows above a question.
    pub(crate) fn last_assistant_text(&self, cx: &App) -> String {
        self.transcript.read(cx).last_assistant_text()
    }

    /// Listens for what the conversation sends the provider, in tests.
    #[cfg(test)]
    pub(crate) fn listen_for_controls(&mut self) -> std::sync::mpsc::Receiver<Control> {
        let (send, receive) = std::sync::mpsc::channel();
        self.controls = Some(send);
        receive
    }

    /// Queues a question from the provider behind the ones already waiting.
    pub(crate) fn push_question(&mut self, request: Question, cx: &mut Context<Self>) {
        let card = cx.new(|cx| crate::questions::QuestionCard::new(request, cx));
        let events = cx.subscribe(&card, |this, _, reply: &crate::questions::Answered, cx| {
            // A reply the conversation no longer waits for needs no further word.
            let _ = this.answer_question(&reply.id, reply.answers.clone(), cx);
        });
        let keep_open = cx.subscribe(
            &card,
            |this, _, request: &crate::questions::KeptOpen, cx| {
                if let Some(controls) = &this.controls {
                    let _ = controls.send(Control {
                        id: request.id.clone(),
                        decision: Decision::KeepOpen,
                    });
                }
                cx.emit(SessionChanged);
                cx.notify();
            },
        );
        self.questions.push_back((card, vec![events, keep_open]));
        self.status = "Waiting for your answer".into();
        cx.emit(QuestionAsked);
    }

    /// Answers the question at the front of the queue: one list of option labels or typed
    /// text per item, or `None` to skip it. The card's buttons and Telegram both end here.
    pub(crate) fn answer_question(
        &mut self,
        id: &str,
        answers: Option<Vec<Vec<String>>>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let items = match self.questions.front() {
            Some((card, _)) if card.read(cx).id() == id => card.read(cx).request().items.len(),
            _ => return Err("That question is no longer waiting for an answer".into()),
        };
        if answers
            .as_ref()
            .is_some_and(|answers| answers.len() != items)
        {
            return Err("The answer does not match the question".into());
        }
        let before = self.needs_approval();
        let outcome = if answers.is_some() {
            Outcome::Answered
        } else {
            Outcome::Skipped
        };
        let decision = answers.map_or(Decision::Deny, Decision::Answer);
        if self.controls.as_ref().is_none_or(|controls| {
            controls
                .send(Control {
                    id: id.to_owned(),
                    decision,
                })
                .is_err()
        }) {
            let message = "The provider is no longer waiting for this answer";
            self.status = message.into();
            cx.notify();
            return Err(message.into());
        }
        if let Some((card, _)) = self.questions.pop_front() {
            self.question_overlay.release(card, cx);
        }
        self.status = if !self.approvals.is_empty() {
            "Waiting for approval"
        } else if !self.questions.is_empty() {
            "Waiting for your answer"
        } else {
            "Working…"
        }
        .into();
        if before != self.needs_approval() {
            cx.emit(SessionChanged);
        }
        cx.emit(QuestionSettled {
            id: id.to_owned(),
            outcome,
        });
        cx.notify();
        Ok(())
    }

    /// The provider withdrew these questions (an auto-skip deadline passed, say).
    pub(super) fn drop_resolved_questions(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        for id in ids {
            let before = self.questions.len();
            self.questions.retain(|(card, _)| card.read(cx).id() != id);
            if self.questions.len() != before {
                cx.emit(QuestionSettled {
                    id,
                    outcome: Outcome::Cancelled,
                });
            }
        }
    }

    /// The turn ended or was stopped: no question waits any longer.
    pub(crate) fn cancel_questions(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .questions
            .drain(..)
            .map(|(card, _)| card.read(cx).id().to_owned())
            .collect();
        for id in ids {
            cx.emit(QuestionSettled {
                id,
                outcome: Outcome::Cancelled,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};
    use monocode_core::{
        agent::{QuestionItem, QuestionOption},
        store::Store,
    };
    use std::{cell::RefCell, rc::Rc, sync::Mutex};

    fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/missing/question-api").unwrap();
        cx.update(|cx| {
            input::bind(cx);
            cx.set_reduce_motion(true);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.loaded = true;
                    view
                })
            })
            .unwrap()
        })
    }

    fn option(label: &str) -> QuestionOption {
        QuestionOption {
            id: label.to_lowercase(),
            label: label.into(),
            description: String::new(),
        }
    }

    fn question(id: &str) -> Question {
        Question {
            id: id.into(),
            title: "A question".into(),
            auto_resolve_at: None,
            items: vec![
                QuestionItem {
                    id: "one".into(),
                    prompt: "Choose one".into(),
                    multiple: false,
                    allow_custom: true,
                    options: vec![option("First"), option("Second")],
                },
                QuestionItem {
                    id: "many".into(),
                    prompt: "Choose several".into(),
                    multiple: true,
                    allow_custom: false,
                    options: vec![option("A"), option("B")],
                },
            ],
        }
    }

    type Settled = Rc<RefCell<Vec<(String, Outcome)>>>;

    /// A conversation with a provider channel and a listener for settled questions.
    fn listening(
        cx: &mut TestAppContext,
    ) -> (
        WindowHandle<Conversation>,
        std::sync::mpsc::Receiver<Control>,
        Settled,
        Subscription,
    ) {
        let window = conversation(cx);
        let (send, receive) = std::sync::mpsc::channel();
        let settled: Settled = Default::default();
        let output = settled.clone();
        let events = window
            .update(cx, |view, _, cx| {
                view.controls = Some(send);
                cx.subscribe(&cx.entity(), move |_, _, event: &QuestionSettled, _| {
                    output.borrow_mut().push((event.id.clone(), event.outcome))
                })
            })
            .unwrap();
        (window, receive, settled, events)
    }

    #[gpui::test]
    fn the_last_assistant_text_is_the_transcripts(cx: &mut TestAppContext) {
        let window = conversation(cx);
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.last_assistant_text(cx), "");
                view.transcript.update(cx, |transcript, cx| {
                    transcript.patch(
                        0,
                        vec![monocode_core::transcript::Row {
                            role: "assistant".into(),
                            text: "Which one?".into(),
                            first: true,
                            sent_at: None,
                        }],
                        cx,
                    )
                });
                assert_eq!(view.last_assistant_text(cx), "Which one?");
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_pending_question_is_the_front_card(cx: &mut TestAppContext) {
        let window = conversation(cx);
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.pending_question(cx), None);
                view.push_question(question("first"), cx);
                view.push_question(question("second"), cx);
                assert_eq!(view.pending_question(cx), Some(question("first")));
                assert!(view.needs_approval());
            })
            .unwrap();
    }

    #[gpui::test]
    fn answering_sends_the_labels_and_settles_the_question(cx: &mut TestAppContext) {
        let (window, controls, settled, _events) = listening(cx);
        window
            .update(cx, |view, _, cx| {
                view.push_question(question("first"), cx);
                view.push_question(question("second"), cx);
                let answers = vec![vec!["Second".to_string()], vec!["A".into(), "B".into()]];
                view.answer_question("first", Some(answers.clone()), cx)
                    .unwrap();
                let control = controls.try_recv().unwrap();
                assert_eq!(control.id, "first");
                let Decision::Answer(sent) = control.decision else {
                    panic!("expected an answer");
                };
                assert_eq!(sent, answers);
                // The next question moves to the front.
                assert_eq!(view.pending_question(cx), Some(question("second")));
                assert_eq!(view.status, "Waiting for your answer");
            })
            .unwrap();
        assert_eq!(
            *settled.borrow(),
            [("first".to_string(), Outcome::Answered)]
        );
    }

    #[gpui::test]
    fn skipping_denies_the_question(cx: &mut TestAppContext) {
        let (window, controls, settled, _events) = listening(cx);
        window
            .update(cx, |view, _, cx| {
                view.push_question(question("only"), cx);
                view.answer_question("only", None, cx).unwrap();
                let control = controls.try_recv().unwrap();
                assert!(matches!(control.decision, Decision::Deny));
                assert_eq!(view.pending_question(cx), None);
                assert!(!view.needs_approval());
                assert_eq!(view.status, "Working…");
            })
            .unwrap();
        assert_eq!(*settled.borrow(), [("only".to_string(), Outcome::Skipped)]);
    }

    #[gpui::test]
    fn an_answer_that_does_not_fit_leaves_the_question_open(cx: &mut TestAppContext) {
        let (window, controls, settled, _events) = listening(cx);
        window
            .update(cx, |view, _, cx| {
                view.push_question(question("first"), cx);
                view.push_question(question("second"), cx);
                // Not the front question.
                assert!(view.answer_question("second", None, cx).is_err());
                assert!(view.answer_question("missing", None, cx).is_err());
                // One answer list for a two-item question.
                let short = Some(vec![vec!["First".to_string()]]);
                assert!(view.answer_question("first", short, cx).is_err());
                assert!(controls.try_recv().is_err());
                assert_eq!(view.pending_question(cx), Some(question("first")));
            })
            .unwrap();
        assert!(settled.borrow().is_empty());
    }

    #[gpui::test]
    fn a_provider_that_stopped_listening_is_reported(cx: &mut TestAppContext) {
        let (window, controls, settled, _events) = listening(cx);
        drop(controls);
        window
            .update(cx, |view, _, cx| {
                view.push_question(question("only"), cx);
                let error = view.answer_question("only", None, cx).unwrap_err();
                assert_eq!(error, "The provider is no longer waiting for this answer");
                assert_eq!(view.status, error);
                assert_eq!(view.pending_question(cx), Some(question("only")));
                view.controls = None;
                assert!(view.answer_question("only", None, cx).is_err());
            })
            .unwrap();
        assert!(settled.borrow().is_empty());
    }

    #[gpui::test]
    fn the_card_and_the_api_share_one_path(cx: &mut TestAppContext) {
        let (window, controls, settled, _events) = listening(cx);
        let card = window
            .update(cx, |view, _, cx| {
                view.push_question(question("first"), cx);
                view.questions.front().unwrap().0.clone()
            })
            .unwrap();
        // The card replies the way its buttons do.
        window
            .update(cx, |_, _, cx| {
                card.update(cx, |_, cx| {
                    cx.emit(crate::questions::Answered {
                        id: "first".into(),
                        answers: Some(vec![vec!["First".to_string()], vec![]]),
                    })
                })
            })
            .unwrap();
        let control = controls.try_recv().unwrap();
        assert!(matches!(control.decision, Decision::Answer(_)));
        // A second reply from the same card is ignored.
        window
            .update(cx, |_, _, cx| {
                card.update(cx, |_, cx| {
                    cx.emit(crate::questions::Answered {
                        id: "first".into(),
                        answers: None,
                    })
                })
            })
            .unwrap();
        assert!(controls.try_recv().is_err());
        assert_eq!(
            *settled.borrow(),
            [("first".to_string(), Outcome::Answered)]
        );
    }

    #[gpui::test]
    fn a_dismissed_card_settles_as_skipped(cx: &mut TestAppContext) {
        let (window, controls, settled, _events) = listening(cx);
        let card = window
            .update(cx, |view, _, cx| {
                view.push_question(question("first"), cx);
                view.questions.front().unwrap().0.clone()
            })
            .unwrap();
        window
            .update(cx, |_, _, cx| {
                card.update(cx, |_, cx| {
                    cx.emit(crate::questions::Answered {
                        id: "first".into(),
                        answers: None,
                    })
                })
            })
            .unwrap();
        assert!(matches!(
            controls.try_recv().unwrap().decision,
            Decision::Deny
        ));
        assert_eq!(*settled.borrow(), [("first".to_string(), Outcome::Skipped)]);
    }

    #[gpui::test]
    fn questions_that_end_without_an_answer_settle_as_cancelled(cx: &mut TestAppContext) {
        let (window, _controls, settled, _events) = listening(cx);
        window
            .update(cx, |view, _, cx| {
                view.push_question(question("a"), cx);
                view.push_question(question("b"), cx);
                view.push_question(question("c"), cx);
                // The provider withdrew one.
                view.drop_resolved_questions(vec!["b".into(), "unknown".into()], cx);
                assert_eq!(view.questions.len(), 2);
                // The turn ended or was stopped.
                view.cancel_questions(cx);
                assert!(view.questions.is_empty());
                assert_eq!(view.pending_question(cx), None);
            })
            .unwrap();
        assert_eq!(
            *settled.borrow(),
            [
                ("b".to_string(), Outcome::Cancelled),
                ("a".to_string(), Outcome::Cancelled),
                ("c".to_string(), Outcome::Cancelled),
            ]
        );
    }
}
