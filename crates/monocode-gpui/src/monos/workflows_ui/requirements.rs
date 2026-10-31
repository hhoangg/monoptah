//! What the machine lacks of what the workflows on screen require: looked up in the background,
//! kept for a minute, and shown as a badge on the list and a section on the workflow page.
use super::super::*;
use super::view::{ERROR, hint, section};
use gpui::{Div, FontWeight};
use monocode_core::mono::workflows::{
    cli::WorkflowHost,
    model::{InputType, InputValue, Workflow, definition_fingerprint},
    preflight::Preflight,
};
use std::time::{Duration, Instant};

/// How long a finding is believed before the workflow is looked at again.
pub(super) const RECHECK: Duration = Duration::from_secs(60);

/// The small label on a workflow whose requirements are not all met.
pub(in crate::monos) fn missing_requirements_badge() -> Div {
    div()
        .debug_selector(|| "mono-workflow-missing-requirements".into())
        .flex_shrink_0()
        .rounded_sm()
        .bg(content(0.1))
        .px_1p5()
        .text_size(px(10.))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(content(0.7))
        .child("Missing requirements")
}

/// What a finding is good for: the definition, and the projects its inputs hold now (they decide
/// where MCP servers are looked up).
fn cache_key(w: &Workflow) -> String {
    let projects: Vec<String> = w
        .inputs
        .iter()
        .filter(|(_, input)| input.ty == InputType::Project)
        .filter_map(|(name, _)| match w.input_values.get(name) {
            Some(InputValue::String(path)) => Some(format!("{name}={path}")),
            _ => None,
        })
        .collect();
    format!("{}|{}", definition_fingerprint(w), projects.join("|"))
}

impl Monos {
    /// What was found missing for `w`, if it was looked at in this definition and something is.
    pub(super) fn missing_requirements(&self, w: &Workflow) -> Option<&Preflight> {
        let (made_for, _, check) = self.wf.preflights.get(&w.id)?;
        (!w.requires.is_empty() && *made_for == cache_key(w) && !check.all_met()).then_some(check)
    }

    /// Looks again, off the UI thread, at the active Mono's workflows that declare requirements
    /// and were not looked at in this definition (and with these projects) or for a minute. One
    /// task checks them one after the other.
    pub(super) fn refresh_preflights(&mut self, cx: &mut Context<Self>) {
        let Some(mono) = self.active.clone() else {
            return;
        };
        let Some(workflows) = self.wf.loaded.get(&mono) else {
            return;
        };
        let ids: Vec<&str> = workflows.iter().map(|w| w.id.as_str()).collect();
        self.wf
            .preflights
            .retain(|id, _| ids.contains(&id.as_str()));
        let due: Vec<(Workflow, String)> = workflows
            .iter()
            .filter(|w| !w.requires.is_empty() && !self.wf.preflighting.contains(&w.id))
            .map(|w| (w.clone(), cache_key(w)))
            .filter(|(w, key)| match self.wf.preflights.get(&w.id) {
                Some((made_for, at, _)) => made_for != key || at.elapsed() >= RECHECK,
                None => true,
            })
            .collect();
        if due.is_empty() {
            return;
        }
        for (w, _) in &due {
            self.wf.preflighting.insert(w.id.clone());
        }
        let mut host = self.workflow_store(&mono);
        let task = cx.background_executor().spawn(async move {
            due.into_iter()
                .map(|(w, key)| {
                    let check = host.preflight(&w);
                    (w.id, key, check)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                for (id, key, check) in found {
                    this.wf.preflighting.remove(&id);
                    this.wf.preflights.insert(id, (key, Instant::now(), check));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The workflow page's list of what is missing, each with its hint.
    pub(super) fn missing_requirements_section(&self, w: &Workflow) -> Option<gpui::Stateful<Div>> {
        let check = self.missing_requirements(w)?;
        let mut block = section("Missing requirements", None)
            .id("mono-workflow-missing")
            .debug_selector(|| "mono-workflow-missing".into());
        for status in check.missing() {
            let requirement = &status.requirement;
            block = block.child(
                div()
                    .px_2()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .child(
                        div()
                            .text_color(gpui::Hsla::from(rgb(ERROR)).opacity(0.8))
                            .child(format!("{} {}", requirement.kind.key(), requirement.name)),
                    )
                    .children(
                        status
                            .detail
                            .clone()
                            .map(|d| div().text_color(content(0.5)).child(d)),
                    )
                    .children(
                        requirement
                            .hint
                            .clone()
                            .map(|h| div().text_color(content(0.4)).child(h)),
                    ),
            );
        }
        Some(block.child(hint("It can't run until these are in place.")))
    }
}
