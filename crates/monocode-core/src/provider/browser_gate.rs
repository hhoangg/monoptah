//! The approval cards of a Mono's Browser gate (`mono::browser::gate`). The gate runs outside the
//! provider process, so its requests do not come from the provider's protocol; this turns each
//! one into the same `Event::Approval` the app already shows on the desktop, the web app and the
//! phone, and answers the gate from the user's decision.
//!
//! The card is shown in every access mode that can answer it, Auto and Full Access included: the
//! gate exists because those modes never ask on their own. A planning turn, a read-only session or
//! a run nobody can answer (no controls) is denied at once.
use super::*;
use crate::mono::browser::gate::{Ask, Category, SUGGEST_ID_PREFIX};
use std::sync::atomic::{AtomicBool, AtomicU64};

static SEQUENCE: AtomicU64 = AtomicU64::new(1);
/// The prefix of a gate approval id, so it can not be mistaken for a provider request id.
pub(super) const ID_PREFIX: &str = "browser-gate-";

/// An open gate request: where the answer goes, and whether the gate has given up waiting.
pub(super) struct Open {
    reply: mpsc::Sender<bool>,
    abandoned: Arc<AtomicBool>,
    never: Arc<AtomicBool>,
}

impl Protocol {
    /// Shows `ask` as an approval card, or denies it. `can_answer` is false when nobody can
    /// answer a card (the run has no controls, or it is being stopped).
    pub(super) fn gate_ask(&mut self, ask: Ask, can_answer: bool) -> Vec<Event> {
        let settings = &self.request.settings;
        if !can_answer || settings.plan || settings.access == AccessMode::ReadOnly {
            let _ = ask.reply.send(false);
            return Vec::new();
        }
        let suggestion = ask.request.category == Category::Suggestion;
        let id = format!(
            "{}{}",
            if suggestion {
                SUGGEST_ID_PREFIX
            } else {
                ID_PREFIX
            },
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let approval = Approval {
            id: id.clone(),
            title: ask.request.title(),
            detail: ask.request.detail(),
            // "Always allow" would turn a one-off question into a standing permission. The
            // suggestion card is the exception: its session answer is "Never for this site",
            // which only the desktop offers (a remote device can allow once or deny).
            can_remember: suggestion,
            can_allow_once: true,
        };
        self.gate_asks.insert(
            id,
            Open {
                reply: ask.reply,
                abandoned: ask.abandoned,
                never: ask.never,
            },
        );
        vec![Event::Approval(approval)]
    }

    /// Delivers the user's decision. False when `id` is not a gate request.
    pub(super) fn gate_respond(&mut self, id: &str, decision: &Decision) -> bool {
        let Some(open) = self.gate_asks.remove(id) else {
            return false;
        };
        if matches!(decision, Decision::Session) {
            open.never.store(true, Ordering::SeqCst);
        }
        let _ = open
            .reply
            .send(matches!(decision, Decision::Allow | Decision::Session));
        true
    }

    /// Withdraws the cards whose request timed out.
    pub(super) fn gate_expire(&mut self) -> Vec<Event> {
        let expired: Vec<String> = self
            .gate_asks
            .iter()
            .filter(|(_, open)| open.abandoned.load(Ordering::SeqCst))
            .map(|(id, _)| id.clone())
            .collect();
        expired
            .into_iter()
            .map(|id| {
                self.gate_asks.remove(&id);
                Event::ApprovalResolved(id)
            })
            .collect()
    }

    /// Denies every open request (the turn ends or is stopped).
    pub(super) fn gate_clear(&mut self) {
        for (_, open) in self.gate_asks.drain() {
            let _ = open.reply.send(false);
        }
    }
}
