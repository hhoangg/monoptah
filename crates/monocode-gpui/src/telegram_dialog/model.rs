//! What the two Telegram dialogs say and decide, without any view: the copy, the delay
//! presets and the state machines of `TelegramProjectDialog.tsx` and
//! `LinkTelegramTopicDialog.tsx`. The views in the parent module only draw this state and run
//! the bridge calls it asks for.
use monocode_core::telegram::{ConnectLink, LinkCode, TelegramStatus};

/// How often an open dialog reads the status or the session's topic again.
pub(crate) const POLL_MS: u64 = 3000;
/// How long the app accepts a link code.
pub(crate) const CODE_TTL_MS: u64 = 15 * 60_000;
pub(crate) const WEBHOOK_PROBLEM: &str = "This bot has a webhook";
pub(crate) const CHANGE_TOKEN_NOTE: &str = "A token for the same bot keeps the group and topic links. A different bot clears the group and topic links, turns Forward questions off, and questions already sent stop working.";
pub(crate) const DISCONNECT_NOTE: &str =
    "Topic links will be lost and questions already sent stop working.";
pub(crate) const OPEN_GROUP_NOTE: &str = "Any member of the group can answer the agent, and a text reply becomes its answer. Keep the group private.";
pub(crate) const ADMIN_NOTE: &str =
    "Make the bot an admin with Manage topics so it can create a topic per session.";
pub(crate) const OFF_HINT: &str = "Forwarding is off. Turn it on to send questions to this group.";
const INTRO_LATER: &str = "Questions that are still unanswered after the chosen time are sent to this group. Any member can answer.";
const INTRO_NOW: &str = "Questions are sent to this group right away. Any member can answer.";

/// The "Send after" presets in seconds.
const DELAY_PRESETS: [(u64, &str); 9] = [
    (0, "Immediately"),
    (30, "30 seconds"),
    (60, "1 minute"),
    (120, "2 minutes"),
    (300, "5 minutes"),
    (600, "10 minutes"),
    (900, "15 minutes"),
    (1800, "30 minutes"),
    (3600, "1 hour"),
];

fn plural(count: u64, unit: &str) -> String {
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// Wording for a stored value that is not one of the presets.
pub(crate) fn custom_delay_label(secs: u64) -> String {
    let text = if secs.is_multiple_of(3600) {
        plural(secs / 3600, "hour")
    } else if secs.is_multiple_of(60) {
        plural(secs / 60, "minute")
    } else {
        plural(secs, "second")
    };
    format!("{text} (custom)")
}

/// The presets, plus the stored value in its place when it is not one of them.
pub(crate) fn delay_options(secs: u64) -> Vec<(u64, String)> {
    let mut options: Vec<(u64, String)> = DELAY_PRESETS
        .iter()
        .map(|(secs, label)| (*secs, (*label).to_string()))
        .collect();
    if !options.iter().any(|(preset, _)| *preset == secs) {
        options.push((secs, custom_delay_label(secs)));
        options.sort_by_key(|(secs, _)| *secs);
    }
    options
}

/// The line under the title.
pub(crate) fn intro(status: Option<&TelegramStatus>) -> &'static str {
    match status {
        Some(status) if status.configured && status.delay_secs == 0 => INTRO_NOW,
        _ => INTRO_LATER,
    }
}

/// "Connected to <group>", never with an empty name.
pub(crate) fn connected_line(status: &TelegramStatus) -> String {
    let title = status.chat_title.as_deref().unwrap_or_default().trim();
    format!(
        "Connected to {}",
        if title.is_empty() {
            "this group"
        } else {
            title
        }
    )
}

/// A yes/no fact Telegram may not have told us yet.
pub(crate) fn fact(value: Option<bool>, on: &'static str, off: &'static str) -> &'static str {
    match value {
        None => "Unknown",
        Some(true) => on,
        Some(false) => off,
    }
}

/// Whether a status change affects if the question forwarder may forward.
pub(crate) fn gate_changed(before: &TelegramStatus, after: &TelegramStatus) -> bool {
    before.configured != after.configured
        || before.enabled != after.enabled
        || before.chat_id != after.chat_id
}

/// The state of the project dialog.
#[derive(Default)]
pub(crate) struct ProjectState {
    pub status: Option<TelegramStatus>,
    pub link: Option<ConnectLink>,
    pub copied: bool,
    pub busy: bool,
    pub error: Option<String>,
    pub changing_token: bool,
    pub confirming_disconnect: bool,
    /// A poll that started before a change must not overwrite the change.
    revision: u64,
    seen: Option<TelegramStatus>,
}

impl ProjectState {
    /// What a poll that starts now must hand back to `poll_done`.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Takes a status and says whether it changed what the forwarder may do.
    fn observe(&mut self, next: TelegramStatus) -> bool {
        let changed = self
            .seen
            .as_ref()
            .is_some_and(|before| gate_changed(before, &next));
        self.seen = Some(next.clone());
        self.status = Some(next);
        changed
    }

    /// A polled status. Returns true when the forwarder's cached status is now stale.
    pub(crate) fn poll_done(&mut self, started: u64, next: TelegramStatus) -> bool {
        started == self.revision && self.observe(next)
    }

    /// Starts an action; false while another one runs.
    pub(crate) fn begin(&mut self) -> bool {
        if self.busy {
            return false;
        }
        self.revision += 1;
        self.busy = true;
        self.error = None;
        true
    }

    pub(crate) fn finish(&mut self, error: Option<String>) {
        self.busy = false;
        if error.is_some() {
            self.error = error;
        }
    }

    /// A new status that an action left behind.
    pub(crate) fn status_set(&mut self, next: TelegramStatus) -> bool {
        self.observe(next)
    }

    pub(crate) fn token_saved(&mut self, next: TelegramStatus) -> bool {
        self.changing_token = false;
        self.observe(next)
    }

    pub(crate) fn disconnected(&mut self, next: TelegramStatus) -> bool {
        self.link = None;
        self.changing_token = false;
        self.confirming_disconnect = false;
        self.observe(next)
    }

    pub(crate) fn edit_token(&mut self) {
        self.confirming_disconnect = false;
        self.error = None;
        self.changing_token = true;
    }

    pub(crate) fn cancel_token_change(&mut self) {
        self.changing_token = false;
        self.error = None;
    }

    pub(crate) fn ask_disconnect(&mut self) {
        self.cancel_token_change();
        self.confirming_disconnect = true;
    }
}

/// The state of the link dialog.
#[derive(Default)]
pub(crate) struct TopicState {
    pub link: Option<LinkCode>,
    pub expired: bool,
    pub linked: bool,
    pub copied: bool,
    pub busy: bool,
    pub error: Option<String>,
    /// The topic the session had when the dialog opened: a successful `/link` changes it.
    baseline: Option<Option<i64>>,
}

impl TopicState {
    pub(crate) fn begin_code(&mut self) -> bool {
        if self.busy {
            return false;
        }
        self.busy = true;
        self.error = None;
        true
    }

    pub(crate) fn code_done(&mut self, result: Result<LinkCode, String>) {
        self.busy = false;
        match result {
            Ok(link) => {
                self.link = Some(link);
                self.expired = false;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub(crate) fn expire(&mut self) {
        if self.link.is_some() {
            self.expired = true;
        }
    }

    /// Takes a read of the session's topic; true when it shows the link just happened.
    pub(crate) fn topic_read(&mut self, topic: Option<i64>) -> bool {
        if self.linked {
            return false;
        }
        let Some(baseline) = self.baseline else {
            self.baseline = Some(topic);
            return false;
        };
        if topic.is_none() || topic == baseline {
            return false;
        }
        self.linked = true;
        true
    }

    /// Whether the "Creating a code..." line shows.
    pub(crate) fn creating(&self) -> bool {
        self.link.is_none() && self.error.is_none() && !self.linked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> TelegramStatus {
        TelegramStatus {
            configured: true,
            bot_username: "alpha_bot".into(),
            delay_secs: 300,
            ..Default::default()
        }
    }
    fn connected() -> TelegramStatus {
        TelegramStatus {
            enabled: true,
            chat_id: Some(-100123),
            chat_title: Some("Alpha team".into()),
            is_forum: Some(true),
            can_manage_topics: Some(true),
            ..configured()
        }
    }
    fn code(text: &str) -> LinkCode {
        LinkCode {
            code: text.into(),
            command: format!("/link@alpha_bot {text}"),
        }
    }
    fn delay_label(secs: u64) -> String {
        delay_options(secs)
            .into_iter()
            .find(|(value, _)| *value == secs)
            .map(|(_, label)| label)
            .unwrap()
    }
    fn labels(secs: u64) -> Vec<String> {
        delay_options(secs).into_iter().map(|(_, l)| l).collect()
    }

    #[test]
    fn the_presets_are_the_nine_the_settings_offer() {
        assert_eq!(
            labels(300),
            [
                "Immediately",
                "30 seconds",
                "1 minute",
                "2 minutes",
                "5 minutes",
                "10 minutes",
                "15 minutes",
                "30 minutes",
                "1 hour",
            ]
        );
        let values: Vec<_> = delay_options(300).into_iter().map(|(s, _)| s).collect();
        assert_eq!(values, [0, 30, 60, 120, 300, 600, 900, 1800, 3600]);
    }

    #[test]
    fn a_stored_value_that_is_not_a_preset_is_a_custom_option_in_order() {
        let options = labels(90);
        assert_eq!(options.len(), 10);
        let one_minute = options.iter().position(|l| l == "1 minute").unwrap();
        assert_eq!(options[one_minute + 1], "90 seconds (custom)");
        assert_eq!(delay_label(90), "90 seconds (custom)");
    }

    #[test]
    fn custom_values_use_the_largest_unit_that_divides_them() {
        assert_eq!(custom_delay_label(1), "1 second (custom)");
        assert_eq!(custom_delay_label(150), "150 seconds (custom)");
        assert_eq!(custom_delay_label(5400), "90 minutes (custom)");
        assert_eq!(custom_delay_label(60 * 7), "7 minutes (custom)");
        assert_eq!(custom_delay_label(7200), "2 hours (custom)");
        assert_eq!(labels(7200).last().unwrap(), "2 hours (custom)");
    }

    #[test]
    fn the_shown_delay_is_the_stored_one() {
        assert_eq!(delay_label(300), "5 minutes");
        assert_eq!(delay_label(30), "30 seconds");
        assert_eq!(delay_label(0), "Immediately");
        assert_eq!(delay_label(3600), "1 hour");
    }

    #[test]
    fn the_intro_says_right_away_only_for_a_configured_zero_delay() {
        let now = "Questions are sent to this group right away. Any member can answer.";
        let later = "Questions that are still unanswered after the chosen time are sent to this group. Any member can answer.";
        assert_eq!(intro(None), later);
        assert_eq!(intro(Some(&TelegramStatus::default())), later);
        assert_eq!(intro(Some(&configured())), later);
        let immediate = TelegramStatus {
            delay_secs: 0,
            ..configured()
        };
        assert_eq!(intro(Some(&immediate)), now);
        let unconfigured = TelegramStatus {
            configured: false,
            delay_secs: 0,
            ..Default::default()
        };
        assert_eq!(intro(Some(&unconfigured)), later);
    }

    #[test]
    fn connected_never_ends_without_a_group_name() {
        assert_eq!(connected_line(&connected()), "Connected to Alpha team");
        for title in [Some(""), Some("   "), None] {
            let status = TelegramStatus {
                chat_title: title.map(str::to_string),
                ..connected()
            };
            assert_eq!(connected_line(&status), "Connected to this group");
        }
    }

    #[test]
    fn unknown_facts_read_unknown() {
        assert_eq!(fact(None, "On", "Off"), "Unknown");
        assert_eq!(fact(Some(true), "On", "Off"), "On");
        assert_eq!(fact(Some(false), "Yes", "No"), "No");
    }

    #[test]
    fn only_configured_enabled_or_chat_changes_open_the_gate() {
        assert!(gate_changed(&configured(), &connected()));
        let disabled = TelegramStatus {
            enabled: false,
            ..connected()
        };
        assert!(gate_changed(&connected(), &disabled));
        assert!(gate_changed(&connected(), &TelegramStatus::default()));
        let renamed = TelegramStatus {
            chat_title: Some("Beta".into()),
            delay_secs: 30,
            problem: Some("x".into()),
            ..connected()
        };
        assert!(!gate_changed(&connected(), &renamed));
    }

    #[test]
    fn a_poll_sees_the_group_connect_and_says_the_cache_is_stale() {
        let mut state = ProjectState::default();
        let started = state.revision();
        // The first status has nothing to compare with.
        assert!(!state.poll_done(started, configured()));
        assert!(state.poll_done(state.revision(), connected()));
        assert_eq!(state.status, Some(connected()));
        assert!(!state.poll_done(state.revision(), connected()));
    }

    #[test]
    fn a_poll_that_started_before_a_change_is_dropped() {
        let mut state = ProjectState::default();
        state.poll_done(state.revision(), connected());
        let stale = state.revision();
        assert!(state.begin());
        state.status_set(TelegramStatus {
            delay_secs: 30,
            ..connected()
        });
        state.finish(None);
        assert!(!state.poll_done(stale, configured()));
        assert_eq!(state.status.as_ref().unwrap().delay_secs, 30);
        assert!(state.status.as_ref().unwrap().chat_id.is_some());
    }

    #[test]
    fn one_action_runs_at_a_time_and_a_new_one_clears_the_error() {
        let mut state = ProjectState::default();
        assert!(state.begin());
        assert!(!state.begin());
        state.finish(Some("Save a bot token first".into()));
        assert!(!state.busy);
        assert_eq!(state.error.as_deref(), Some("Save a bot token first"));
        assert!(state.begin());
        assert_eq!(state.error, None);
    }

    #[test]
    fn a_rejected_token_keeps_the_connection_and_the_field() {
        let mut state = ProjectState::default();
        state.poll_done(0, connected());
        state.edit_token();
        assert!(state.begin());
        state.finish(Some("Bot token was rejected".into()));
        assert!(state.changing_token);
        assert_eq!(state.status, Some(connected()));
        assert_eq!(state.error.as_deref(), Some("Bot token was rejected"));
        // A good one closes the field and shows the new status.
        assert!(state.begin());
        assert!(!state.token_saved(connected()));
        state.finish(None);
        assert!(!state.changing_token);
        assert_eq!(state.error, None);
    }

    #[test]
    fn another_bot_clears_the_group_and_says_so_to_the_cache() {
        let mut state = ProjectState::default();
        state.poll_done(0, connected());
        state.edit_token();
        let other = TelegramStatus {
            bot_username: "beta_bot".into(),
            ..configured()
        };
        assert!(state.token_saved(other.clone()));
        assert_eq!(state.status, Some(other));
    }

    #[test]
    fn cancelling_a_token_change_drops_the_field_and_the_error() {
        let mut state = ProjectState::default();
        state.edit_token();
        state.error = Some("x".into());
        state.cancel_token_change();
        assert!(!state.changing_token);
        assert_eq!(state.error, None);
    }

    #[test]
    fn disconnect_asks_first_and_cancel_keeps_the_connection() {
        let mut state = ProjectState::default();
        state.poll_done(0, connected());
        state.edit_token();
        state.ask_disconnect();
        assert!(state.confirming_disconnect);
        assert!(!state.changing_token);
        state.confirming_disconnect = false;
        assert_eq!(state.status, Some(connected()));
        state.ask_disconnect();
        state.link = Some(ConnectLink {
            code: "C".into(),
            url: "u".into(),
            fallback: "f".into(),
        });
        assert!(state.disconnected(TelegramStatus::default()));
        assert!(!state.confirming_disconnect);
        assert!(state.link.is_none());
        assert!(!state.status.as_ref().unwrap().configured);
    }

    #[test]
    fn editing_the_token_cancels_a_pending_disconnect() {
        let mut state = ProjectState::default();
        state.ask_disconnect();
        state.error = Some("x".into());
        state.edit_token();
        assert!(!state.confirming_disconnect);
        assert!(state.changing_token);
        assert_eq!(state.error, None);
    }

    #[test]
    fn the_code_shows_a_command_and_creating_goes_away() {
        let mut state = TopicState::default();
        assert!(state.creating());
        assert!(state.begin_code());
        assert!(!state.begin_code());
        state.code_done(Ok(code("CODE1")));
        assert_eq!(
            state.link.as_ref().unwrap().command,
            "/link@alpha_bot CODE1"
        );
        assert!(!state.creating());
        assert!(!state.busy);
    }

    #[test]
    fn a_code_that_cannot_be_made_shows_the_error_and_no_command() {
        let mut state = TopicState::default();
        state.begin_code();
        state.code_done(Err("Telegram is not connected".into()));
        assert_eq!(state.error.as_deref(), Some("Telegram is not connected"));
        assert!(state.link.is_none());
        assert!(!state.creating());
        // Trying again clears the error.
        assert!(state.begin_code());
        assert_eq!(state.error, None);
    }

    #[test]
    fn the_same_binding_is_not_a_new_link() {
        let mut state = TopicState::default();
        assert!(!state.topic_read(Some(11)));
        assert!(!state.topic_read(Some(11)));
        assert!(!state.linked);
        assert!(state.topic_read(Some(42)));
        assert!(state.linked);
        assert!(!state.creating());
    }

    #[test]
    fn a_session_without_a_topic_that_gets_one_is_linked() {
        let mut state = TopicState::default();
        assert!(!state.topic_read(None));
        assert!(!state.topic_read(None));
        assert!(state.topic_read(Some(7)));
    }

    #[test]
    fn a_vanished_topic_is_not_a_link() {
        let mut state = TopicState::default();
        state.topic_read(Some(5));
        assert!(!state.topic_read(None));
        assert!(!state.linked);
    }

    #[test]
    fn linking_is_reported_once() {
        let mut state = TopicState::default();
        state.topic_read(None);
        assert!(state.topic_read(Some(1)));
        assert!(!state.topic_read(Some(2)));
    }

    #[test]
    fn the_code_expires_after_fifteen_minutes_and_a_new_one_revives_it() {
        assert_eq!(CODE_TTL_MS, 15 * 60_000);
        let mut state = TopicState::default();
        state.expire();
        assert!(!state.expired, "no code, nothing to expire");
        state.code_done(Ok(code("CODE1")));
        state.expire();
        assert!(state.expired);
        assert!(state.begin_code());
        state.code_done(Ok(code("CODE2")));
        assert!(!state.expired);
        assert_eq!(state.link.as_ref().unwrap().code, "CODE2");
    }
}
