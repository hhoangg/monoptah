//! The question API of a conversation: queue the questions the provider asks, answer the one
//! in front, and drop the ones that are no longer waiting.
use super::*;
use monocode_core::agent::Question;

impl Conversation {
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
    }

    /// Answers the question at the front of the queue: one list of option labels or typed
    /// text per item, or `None` to skip it. The card's buttons end here.
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
        cx.notify();
        Ok(())
    }

    /// The provider withdrew these questions (an auto-skip deadline passed, say).
    pub(super) fn drop_resolved_questions(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        for id in ids {
            self.questions.retain(|(card, _)| card.read(cx).id() != id);
        }
    }

    /// The turn ended or was stopped: no question waits any longer.
    pub(crate) fn cancel_questions(&mut self) {
        self.questions.clear();
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
    use std::sync::Mutex;

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

    /// A conversation with a provider channel.
    fn listening(
        cx: &mut TestAppContext,
    ) -> (
        WindowHandle<Conversation>,
        std::sync::mpsc::Receiver<Control>,
    ) {
        let window = conversation(cx);
        let (send, receive) = std::sync::mpsc::channel();
        window
            .update(cx, |view, _, _| view.controls = Some(send))
            .unwrap();
        (window, receive)
    }

    #[gpui::test]
    fn answering_sends_the_labels_and_settles_the_question(cx: &mut TestAppContext) {
        let (window, controls) = listening(cx);
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
                assert_eq!(view.questions.front().unwrap().0.read(cx).id(), "second");
                assert_eq!(view.status, "Waiting for your answer");
            })
            .unwrap();
    }

    #[gpui::test]
    fn skipping_denies_the_question(cx: &mut TestAppContext) {
        let (window, controls) = listening(cx);
        window
            .update(cx, |view, _, cx| {
                view.push_question(question("only"), cx);
                view.answer_question("only", None, cx).unwrap();
                let control = controls.try_recv().unwrap();
                assert!(matches!(control.decision, Decision::Deny));
                assert!(view.questions.is_empty());
                assert!(!view.needs_approval());
                assert_eq!(view.status, "Working…");
            })
            .unwrap();
    }

    #[gpui::test]
    fn an_answer_that_does_not_fit_leaves_the_question_open(cx: &mut TestAppContext) {
        let (window, controls) = listening(cx);
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
                assert_eq!(view.questions.front().unwrap().0.read(cx).id(), "first");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_provider_that_stopped_listening_is_reported(cx: &mut TestAppContext) {
        let (window, controls) = listening(cx);
        drop(controls);
        window
            .update(cx, |view, _, cx| {
                view.push_question(question("only"), cx);
                let error = view.answer_question("only", None, cx).unwrap_err();
                assert_eq!(error, "The provider is no longer waiting for this answer");
                assert_eq!(view.status, error);
                assert_eq!(view.questions.front().unwrap().0.read(cx).id(), "only");
                view.controls = None;
                assert!(view.answer_question("only", None, cx).is_err());
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_card_and_the_api_share_one_path(cx: &mut TestAppContext) {
        let (window, controls) = listening(cx);
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
    }

    #[gpui::test]
    fn a_dismissed_card_settles_as_skipped(cx: &mut TestAppContext) {
        let (window, controls) = listening(cx);
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
    }

    #[gpui::test]
    fn questions_that_end_without_an_answer_are_dropped(cx: &mut TestAppContext) {
        let (window, _controls) = listening(cx);
        window
            .update(cx, |view, _, cx| {
                view.push_question(question("a"), cx);
                view.push_question(question("b"), cx);
                view.push_question(question("c"), cx);
                // The provider withdrew one.
                view.drop_resolved_questions(vec!["b".into(), "unknown".into()], cx);
                assert_eq!(view.questions.len(), 2);
                // The turn ended or was stopped.
                view.cancel_questions();
                assert!(view.questions.is_empty());
            })
            .unwrap();
    }
}
