//! The cloud mark: tells a session that runs in a GitHub codespace or a Docker sandbox from a
//! local one wherever sessions are listed (sidebar rows, title tabs, a Mono's sub-agents and
//! launch cards, search results), and the open session's workspace bar shows the backend and the
//! workspace's state beside it.
//!
//! Nothing here reads state: the shell owns the cached workspace states (`Shell::cloud_states`,
//! read without ever waking a workspace) and hands each view a [`CloudBadge`].
use crate::theme::*;
use gpui::{
    AnyElement, Context, Div, ElementId, Hsla, IntoElement, Render, Role, SharedString, Stateful,
    Window, div, prelude::*, px, rgb,
};
use monocode_core::cloud_sessions::{CloudBackendKind, CloudState};

/// How a workspace state reads: green while it runs, amber while it changes, red when it is
/// gone or failed, muted when it is stopped or could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Good,
    Busy,
    Idle,
    Bad,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Indicator {
    pub label: &'static str,
    pub tone: Tone,
}

/// Row indicator for a codespace state. A state that could not be read (offline,
/// gh missing, not signed in) is "Not connected": a state, never an error toast.
pub(crate) fn indicator(state: Option<CloudState>) -> Indicator {
    let (label, tone) = match state {
        Some(CloudState::Available) => ("Available", Tone::Good),
        Some(CloudState::Starting) => ("Starting", Tone::Busy),
        Some(CloudState::Stopping) => ("Stopping", Tone::Busy),
        Some(CloudState::Stopped) => ("Stopped", Tone::Idle),
        Some(CloudState::Unavailable) => ("Unavailable", Tone::Bad),
        Some(CloudState::Unknown) | None => ("Not connected", Tone::Idle),
    };
    Indicator { label, tone }
}

/// The state dot's colour.
pub(crate) fn tone_color(tone: Tone) -> Hsla {
    match tone {
        Tone::Good => rgb(0x4ade80).into(),
        Tone::Busy => rgb(0xfbbf24).into(),
        Tone::Bad => rgb(0xf87171).into(),
        Tone::Idle => content(0.35),
    }
}

/// The backend's name as the cloud target menu words it.
pub(crate) fn backend_name(kind: CloudBackendKind) -> &'static str {
    match kind {
        CloudBackendKind::Codespaces => "Codespace",
        CloudBackendKind::DockerSandbox => "Docker Sandbox",
    }
}

/// What a view needs to mark a cloud session: its backend, its workspace and how that stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloudBadge {
    pub kind: CloudBackendKind,
    /// The codespace or sandbox name.
    pub instance: String,
    /// The workspace's cached state, or its startup while that runs or failed.
    pub indicator: Indicator,
}

impl CloudBadge {
    pub(crate) fn backend(&self) -> &'static str {
        backend_name(self.kind)
    }
    /// `Codespace · Available`.
    pub(crate) fn status(&self) -> String {
        format!("{} \u{b7} {}", self.backend(), self.indicator.label)
    }
    /// `Codespace · fuzzy-space-xyz · main`: the backend, the workspace and, when known, the
    /// branch the session works on.
    pub(crate) fn tooltip(&self, branch: Option<&str>) -> String {
        let mut tip = self.backend().to_owned();
        if !self.instance.is_empty() {
            tip.push_str(" \u{b7} ");
            tip.push_str(&self.instance);
        }
        if let Some(branch) = branch.filter(|b| !b.is_empty()) {
            tip.push_str(" \u{b7} ");
            tip.push_str(branch);
        }
        tip
    }
}

/// A plain text hover card, as the cloud panel's.
pub(crate) struct CloudTip(pub SharedString);
impl Render for CloudTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(320.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(shell_fill())
            .border_1()
            .border_color(content(0.12))
            .text_size(px(12.))
            .text_color(content(0.85))
            .child(self.0.clone())
    }
}

/// The cloud mark, the same on every surface that lists sessions (sidebar rows, title tabs,
/// split headers, a Mono's sub-agents and launch cards, search results): the cloud glyph and the
/// workspace state's dot, and in roomy places the state's label. Hovering names the backend,
/// the workspace and its state. `selector` is its debug selector.
pub(crate) fn badge(
    id: impl Into<ElementId>,
    selector: String,
    badge: &CloudBadge,
    compact: bool,
) -> Stateful<Div> {
    let indicator = badge.indicator;
    let tip: SharedString = format!("{} \u{b7} {}", badge.tooltip(None), indicator.label).into();
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .role(Role::Image)
        .aria_label(format!(
            "{} {}: {}",
            badge.kind.title(),
            badge.instance,
            indicator.label
        ))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(px(11.))
        .text_color(content(0.5))
        .child(icon("cloud", 12.).text_color(content(0.55)))
        .child(
            div()
                .size(px(6.))
                .rounded_full()
                .bg(tone_color(indicator.tone)),
        )
        .when(!compact, |d| d.child(indicator.label))
        .tooltip(move |_, cx| cx.new(|_| CloudTip(tip.clone())).into())
}

/// The open session's cloud identity: the glyph, the backend and the workspace state with its
/// dot, as `Codespace · Available`. Sized like the workspace bar's other triggers.
pub(crate) fn status(
    id: impl Into<ElementId>,
    badge: &CloudBadge,
    branch: Option<&str>,
) -> AnyElement {
    let tip: SharedString = badge.tooltip(branch).into();
    let status = badge.status();
    div()
        .id(id)
        .debug_selector(|| "cloud-session-status".into())
        .role(Role::Status)
        .aria_label(format!("{status}, {tip}"))
        .flex_shrink_0()
        .h(px(24.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(content(0.55))
        .child(icon("cloud", 14.).text_color(content(0.6)))
        .child(status)
        .child(
            div()
                .size(px(6.))
                .rounded_full()
                .bg(tone_color(badge.indicator.tone)),
        )
        .tooltip(move |_, cx| cx.new(|_| CloudTip(tip.clone())).into())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn badge(kind: CloudBackendKind, state: Option<CloudState>) -> CloudBadge {
        CloudBadge {
            kind,
            instance: "fuzzy-space-xyz".into(),
            indicator: indicator(state),
        }
    }

    #[test]
    fn status_names_the_backend_and_the_workspace_state() {
        assert_eq!(
            badge(CloudBackendKind::Codespaces, Some(CloudState::Available)).status(),
            "Codespace \u{b7} Available"
        );
        assert_eq!(
            badge(CloudBackendKind::DockerSandbox, Some(CloudState::Stopped)).status(),
            "Docker Sandbox \u{b7} Stopped"
        );
        assert_eq!(
            badge(CloudBackendKind::Codespaces, Some(CloudState::Starting)).status(),
            "Codespace \u{b7} Starting"
        );
        assert_eq!(
            badge(CloudBackendKind::Codespaces, None).status(),
            "Codespace \u{b7} Not connected"
        );
    }

    #[test]
    fn tooltip_names_the_backend_the_instance_and_a_known_branch() {
        let badge = badge(CloudBackendKind::Codespaces, Some(CloudState::Available));
        assert_eq!(badge.tooltip(None), "Codespace \u{b7} fuzzy-space-xyz");
        assert_eq!(badge.tooltip(Some("")), "Codespace \u{b7} fuzzy-space-xyz");
        assert_eq!(
            badge.tooltip(Some("main")),
            "Codespace \u{b7} fuzzy-space-xyz \u{b7} main"
        );
    }
}
